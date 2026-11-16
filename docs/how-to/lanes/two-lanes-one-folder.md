---
title: Run a folder as two lanes
description: Add a second lane to a worktree, with its own ports, hostnames and data, and remove it when you are done.
kind: how-to
---

Use this when you want the same worktree running twice, for example against empty data or with other services, without making a second worktree.

## Add the lane

Run this in the folder. The new lane takes the lowest free lane number, such as lane 2 with hostnames under `myapp2.localhost`.

```console title="~/dev/myapp"
$ devlanes lane new --name review
```

The name is what you type to reach the lane. Output and the status show it by its number.

## Start it

Commands in the folder keep acting on its first lane, so name the second one. Add `--profile` with one of your profiles, the named sets of services and settings in the project file, to run other services than the first lane.

```console title="~/dev/myapp"
$ devlanes start --lane review --no-ui
```

The lane runs its setup and create tasks like any new lane, and a unit reference such as review:web reaches one of its services.

## Keep the lanes' files apart

Each lane has its own ports, hostnames and data folder, but the files in the worktree are shared, build output and caches included. Point a service that writes while it runs at the lane's data folder.

```yaml devlanes
services:
  api:
    env:
      UPLOAD_DIR: "${{ lane.data_path }}/uploads"
```

## Remove it

Cleaning the lane deletes its data and frees its number. A lane added with `devlanes lane new` does not come back afterwards.

```console title="~/dev/myapp"
$ devlanes lane clean review --yes
```

## If you need another branch

The lanes of one folder run the branch it has checked out. For another branch, add a worktree with Git and start there, as [Every worktree gets a lane](../../learn/worktrees.md) shows.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Remove lanes](clean-up.md)
