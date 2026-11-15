---
title: Copy build output to a fixed folder
description: Copy a folder a service builds, such as an unpacked browser extension, to a fixed path on this machine or from a box to yours.
kind: how-to
---

When another program loads a folder your service builds from a path it remembers, such as a browser with an unpacked extension, devlanes can copy that folder to a fixed place and keep the copy current while the service runs.

## Name the folder to copy

Add the build folder to the service that writes it. The path starts at the project file's folder in the lane's worktree:

```yaml devlanes
services:
  web:
    run_script:
      file: web/package.json
      name: dev
    sync:
      - from: web/.output/chrome-mv3
        ignore: ["*.map"]
```

The copy starts once the service starts and follows each change the build makes, deletions included, until the service stops. Files that match an ignore pattern stay out of it.

## Load the copy

Without a destination, each lane's copy lands in a folder named after the project, the lane and the source folder: `~/devlanes/sync/myapp/main/chrome-mv3` for lane 0 and `~/devlanes/sync/myapp/lane1/chrome-mv3` for lane 1. Point the browser at that folder once. The path stays the same across rebuilds and restarts, and it does not depend on where the worktree is.

devlanes marks the folder as its own. It refuses to write into an existing folder that holds files it did not create, so a folder you made by hand has to be moved away first.

## Choose the destination

A path of your own goes in the destination field. A leading `~/` is your home folder, and any other relative path starts at the project file's folder:

```yaml devlanes
services:
  web:
    sync:
      - from: web/.output/chrome-mv3
        to: ~/extensions/myapp-${{ lane.name }}
```

Keep the lane's name in the path, so each lane writes a folder of its own.

## From a box to your machine

When the service runs on a box you are connected to, your machine gets its own copy while the box's lane runs. It lands in the devlanes home on your machine, under the box's name: `~/.devlanes/sync/my-vps/myapp/myapp@1/chrome-mv3` for lane 1 on the box named my-vps. The destination field is ignored there, so the box cannot choose where your machine writes.

The two defaults differ by a dot: the local copy goes under `~/devlanes`, the copy from a box under `~/.devlanes`.

One command lists the copies of running lanes and the state of each:

```console title="on your machine"
$ devlanes remote sync
```

A hidden service is not copied to your machine, and large files are skipped. [Remote mirroring](../../concepts/remote-mirroring.md) covers how the copy travels and its limits.

## Related

- [Connect to a box](../remote/connect.md)
- [Choose what a box shares](../remote/what-a-box-shares.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
