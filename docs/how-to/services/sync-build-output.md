---
title: Copy build output to a fixed folder
description: Copy a folder a service builds, such as an unpacked browser extension, to a fixed path on this machine or from a box to yours.
kind: how-to
---

When another program loads a folder your service builds from a path it remembers, such as a browser with an unpacked extension, devlanes can copy that folder to a fixed place and keep the copy current while the service runs.

::example{slug="remote-docker-box"}

## Add the folder to the service

```yaml devlanes
services:
  web:
    run_script:
      file: web/package.json
      name: dev
    sync:
      - from: web/.output/chrome-mv3
        to: ${{ lane.data_path }}/chrome-mv3
        ignore: ["*.map"]
```

The source path starts at the project file's folder. The destination here is in the lane's data folder, a folder devlanes keeps for each lane outside the worktree, so each lane has its own copy. The copy starts with the service, so restart a service that already runs:

```console title="~/dev/myapp"
$ devlanes restart web --no-ui
```

## Load the copy

This prints the lane's data folder as `DEVLANES_LANE_DATA`; the copy is the chrome-mv3 folder inside it:

```console title="~/dev/myapp"
$ devlanes env show web
```

Point the browser at that folder once. It stays current across rebuilds and restarts, and cleaning the lane deletes it. Leave the empty `.devlanes-sync` file in it; it marks the folder as devlanes' own.

## If you want another destination

Give an absolute path, or one relative to the project file's folder. A path that starts with `~/` and the default destination need the `HOME` variable, which PowerShell and cmd do not set.

## If the service runs on a box

When your machine is connected to a box, another machine whose lanes you open from yours, it gets its own copy in the devlanes home under the box's name, and ignores the destination in the project file. [Remote mirroring](../../concepts/remote-mirroring.md) covers it.

## Related

- [Connect to a box](../remote/connect.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
