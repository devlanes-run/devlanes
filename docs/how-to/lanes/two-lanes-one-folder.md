---
title: Run a folder as two lanes
description: Add a second lane to a worktree, with its own ports, hostnames and data, and remove it when you are done.
kind: how-to
---

Use this when you want the same checkout running twice, for example with another profile or against empty data, and a second worktree would only repeat the same files.

## Add the lane

Run `devlanes lane new --name review` in the folder, here the worktree of the main branch. The new lane takes the lowest free lane number, so with lanes 0 and 1 in use it becomes lane 2 and its hostnames end in `myapp2.localhost`. On a terminal the command asks whether to start the lane right away.

A lane added this way never becomes lane 0, even in the folder of the primary branch. The folder's first lane keeps its number and keeps running.

## Name it

The name is how commands reach the lane. Without one, the lane is called after the folder: myapp-2, then myapp-3. A name uses letters, digits, hyphens, underscores and dots. It cannot be a number, main, or lane followed by a number, since those already name lanes.

Inside the lane the name does not show. Its `lane.name` and `DEVLANES_LANE_NAME` are lane2, after the number, as for any other lane, so values built from them stay apart ([Lanes and projects](../../concepts/lanes-and-projects.md) explains lane names).

## Start it and reach its units

Commands typed in the folder keep acting on its first lane. The second one needs its name, given with the global `--lane` flag or as a prefix on a unit, such as review:web for its web service. Pass the name to a start, and add one of your profiles with `--profile` to run the lane with other services or settings than the first one.

The start uses the environment of the shell you typed it in, as any start does. [Lane and unit references](../../reference/lane-and-unit-references.md) lists the other forms a reference can take.

## Keep the lanes' files apart

Each lane gets its own ports, hostnames and data folder, a folder devlanes keeps for the lane outside the worktree. Everything inside the worktree is shared: installed dependencies, build output, a framework's cache such as Vite's or Wrangler's, and any file a service writes next to the code. Point a service that writes while it runs at the lane's data folder.

```yaml devlanes
services:
  api:
    env:
      UPLOAD_DIR: "${{ lane.data_path }}/uploads"
```

Each lane then has an uploads folder of its own. [Files and folders](../../reference/files.md) says where the data folders are.

## Remove it

`devlanes lane clean review --yes` stops the lane, runs its clean tasks and teardown, deletes its data folder and logs, and frees its number and ports. A lane added with `devlanes lane new` does not come back after that. Adding one under the same name again gives a new lane with an empty data folder. [Remove lanes](clean-up.md) covers cleaning in full.

## If you need another branch

The lanes of one folder run the branch the folder has checked out. For a branch of its own, add a worktree with Git and start there, as [Every worktree gets a lane](../../learn/worktrees.md) shows.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Remove lanes](clean-up.md)
