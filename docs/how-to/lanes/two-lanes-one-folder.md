---
title: Run a folder as two lanes
description: Add a second lane to a worktree, with its own ports, hostnames and data, and remove it when you are done.
kind: how-to
---

Use this when you want the same worktree running twice, for example against empty data or with other services, and a second worktree would only repeat the same files.

## Add the lane

Run the command in the folder, here the worktree of the main branch. On a terminal it asks whether to start the new lane right away.

```console title="~/dev/myapp"
$ devlanes lane new --name review
```

The lane takes the lowest free lane number, so with lanes 0 and 1 in use it becomes lane 2 and its hostnames end in `myapp2.localhost`. A lane added this way never becomes lane 0, even in the folder of the primary branch.

## Name it

The name is how commands reach the lane. Without one, the lane is called after the folder: myapp-2, then myapp-3. A name uses letters, digits, hyphens, underscores and dots. It cannot be a number, main, lane followed by a number, or the name or branch of another lane of the project.

Inside the lane the name does not show. Its `lane.name` and `DEVLANES_LANE_NAME` are lane2, after the number, as for any other lane, so values built from them stay apart ([Lanes and projects](../../concepts/lanes-and-projects.md) explains lane names).

## Start it

Commands typed in the folder keep acting on its first lane, so the start names the second one with the global `--lane` flag. To run it with other services or settings than the first lane, add `--profile` with one of the profiles in your project file, the named sets of services and settings a start can pick.

```console title="~/dev/myapp"
$ devlanes start --lane review --no-ui
```

The start uses the environment of the shell you type it in, as any start does. The name also works as a prefix on a unit, such as review:web for the lane's web service; [Lane and unit references](../../reference/lane-and-unit-references.md) lists the other forms.

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

Cleaning the lane deletes its data folder, its logs and its own containers with their volumes, and frees its number.

```console title="~/dev/myapp"
$ devlanes lane clean review --yes
```

A lane added with `devlanes lane new` does not come back after a clean. Adding one under the same name again gives a new, empty lane. [Remove lanes](clean-up.md) says what a clean deletes and when its clean tasks run.

## If you need another branch

The lanes of one folder run the branch the folder has checked out. For a branch of its own, add a worktree with Git and start there, as [Every worktree gets a lane](../../learn/worktrees.md) shows.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Remove lanes](clean-up.md)
