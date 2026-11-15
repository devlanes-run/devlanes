---
title: Keep personal changes out of Git
description: Turn services off, change variables and add services on your machine only, in a local file next to the project file.
kind: how-to
---

Use this guide when you want the project to run a little differently on your machine, such as one service off or a debug log level, without changing the project file the team shares.

::example{slug="workflows-dev-and-prod"}

## Create the local file

The local file is `devlanes.local.yaml`, next to `devlanes.yaml`. It takes the same keys as the project file and holds only what you change. devlanes merges it over the project file, so a mapping such as a service's variables keeps the entries you leave out:

```yaml devlanes
# devlanes.local.yaml
services:
  web:
    autostart: false
  api:
    env:
      LOG_LEVEL: debug
```

Here a plain start leaves the web service out unless the lane ran it before or a profile names it, and the API gets one more variable while keeping the rest. A list or a single value replaces the project file's instead of adding to it. [Config files and reload](../../concepts/config-and-reload.md#files-merge-in-a-fixed-order) has the merge rules.

The local file cannot set `project` or a file-level `extends`, and devlanes names the line when it does. It cannot delete what the project file declares either: turn a service off with autostart as above, and empty a list by setting it to an empty one.

## Keep it out of Git

devlanes does not touch your `.gitignore`, so add the file to it yourself:

```text title=".gitignore"
devlanes*.local.yaml
```

The pattern covers `devlanes.prod.local.yaml` as well.

## Add what only you run

A service that exists only in the local file needs its own command or script. A server you start yourself, such as a database admin tool, goes under `infra.external`, which gives it a hostname without devlanes running it:

```yaml devlanes
# devlanes.local.yaml
services:
  scratch:
    run_script:
      file: apps/scratch/package.json
      name: dev
    hosts: "scratch.${{ lane.domain }}"
infra:
  external:
    pgadmin:
      address: 127.0.0.1:5050
      hosts: "pgadmin.${{ lane.domain }}"
```

[Give a server you run yourself a hostname](../services/external.md) covers the second entry.

## Apply the change

A lane whose worktree has no local file, such as a new one, reads the primary worktree's, as [Files and folders](../../reference/files.md#files-of-a-project) describes. One file there reaches each such lane of the project.

An edit to the local file leaves running processes alone, though new hostnames are routed at once. `devlanes status` shows the lanes whose config changed, and `devlanes reload` applies the change to them. An error in the file is reported with the local file's name and line.

## If one worktree needs other changes

Create a local file in that worktree. Its lanes then read that file instead of the primary worktree's, and the two are not merged, so copy over what you want to keep.

Some settings apply to the whole project, and devlanes warns when a worktree's file sets them differently. For `lanes.max` and `lanes.primary` the primary worktree's value applies. Shared containers are started by every lane, so a lane whose file defines them differently changes them for all lanes when it starts.

## If you use a second project file

Each project file has its own local file, named after it: `devlanes.prod.yaml` reads `devlanes.prod.local.yaml`.

A project file that extends the main one also picks up your main local file, merged right after the file it belongs to. Your personal changes then reach both projects. [Run dev servers and a production build side by side](dev-and-prod.md) sets up such a pair.

## Related

- [Start a service only on demand](../services/on-demand.md)
- [Share settings and choose which services start](templates-and-profiles.md)
- [Config files and reload](../../concepts/config-and-reload.md)
