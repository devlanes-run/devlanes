---
title: Copy build output to a fixed folder
description: Copy a folder a service builds, such as an unpacked browser extension, to a fixed path on this machine or from a box to yours.
kind: how-to
---

When another program loads a folder your service builds from a path it remembers, such as a browser with an unpacked extension, devlanes can copy that folder to a fixed place and keep the copy current while the service runs.

::example{slug="remote-docker-box"}

## Add the folder to the service

Name the build folder on the service that writes it, with the destination. The source path starts at the project file's folder in the lane's worktree:

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

The destination here sits in the lane's data folder, a folder devlanes keeps for each lane outside the worktree, so each lane gets a copy of its own and the path does not move with the worktree. Files that match an ignore pattern stay out of the copy.

The copy starts when the service starts, so a service that already runs picks up the entry at its next restart:

```console title="~/dev/myapp"
$ devlanes restart web --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  web  http://127.0.0.1:20070
```

From then on the copy follows each change the build makes, deletions included, until the service stops, and it stays on disk after that.

Point the browser, or whatever loads the folder, at the destination once; the path stays the same across rebuilds and restarts. Cleaning the lane deletes the folder, and the next lane gets a new one. The service's environment holds the data folder's path:

```console title="on Windows 11"
$ devlanes env show web
DEVLANES_LANE_DATA=C:/Users/you/.devlanes/data/myapp/217
```

The copy is the folder named chrome-mv3 inside it. Next to the copied files sits an empty file, `.devlanes-sync`, which marks the folder as devlanes' own; leave it there.

```console title="on Windows 11"
$ powershell.exe -NoProfile -Command "Get-ChildItem C:/Users/you/.devlanes/data/myapp/217/chrome-mv3"
    Directory: C:\Users\you\.devlanes\data\myapp\217\chrome-mv3

Mode                 LastWriteTime         Length Name
----                 -------------         ------ ----
-a----        11/15/2026   8:52 PM              0 .devlanes-sync
-a----        11/15/2026   8:52 PM             55 manifest.json
```

The build also writes a map file, which the ignore pattern keeps out of the copy. To see whether the copy runs, list the copies of the running lanes and look for its state:

```console title="on Windows 11"
$ devlanes remote sync
items:
  lane: myapp@0
  unit: web
  kind: local
  local:
    to: C:/Users/you/.devlanes/data/myapp/217/chrome-mv3
    status:
      state: watching
      files: 1
remote: -
```

Watching means the copy is up to date and follows the build. The list also holds the copies from connected boxes.

## If you want the copy somewhere else

A destination can also be an absolute path, or one relative to the project file's folder. Without a destination, the copy goes to a folder under `~/devlanes/sync` named after the project, the lane and the source folder. That default, and a destination that is a lone tilde or starts with a tilde and a slash, read the `HOME` variable. PowerShell and cmd do not set it, so a start from them fails on the entry.

devlanes refuses to write into an existing folder that holds files but no marker file, so move a folder you made by hand out of the way first.

## If the service runs on a box

When the service runs on a box, another machine whose lanes you open from yours, and you are connected to it, your machine gets its own copy while the box's lane runs. It lands in the devlanes home on your machine, under the box's name, such as `sync/my-vps/myapp/myapp@1/chrome-mv3` for lane 1 on the box named my-vps. The destination in the project file is ignored there, so the box cannot choose where your machine writes.

The devlanes home is `~/.devlanes` unless the `DEVLANES_HOME` variable moves it.

A service the box keeps to itself with `hidden` is not copied to your machine, and files past a size limit are skipped; [Remote mirroring](../../concepts/remote-mirroring.md) covers how the copy travels.

## Related

- [Connect to a box](../remote/connect.md)
- [Choose what a box shares](../remote/what-a-box-shares.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
