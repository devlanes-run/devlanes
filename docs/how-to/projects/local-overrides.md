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

## Keep it out of Git

devlanes does not touch your `.gitignore`, so add the file to it yourself:

```text title=".gitignore"
devlanes.local.yaml
*.local.yaml
```

The second line also covers the local files of other project files, such as `devlanes.prod.local.yaml` for `devlanes.prod.yaml`.

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

## Leave the project's name and files alone

The local file cannot set `project` or a file-level `extends`, and devlanes reports the line when it does. The name has to be the same on every machine, so every developer's lanes get the same domains and container names.

Nothing in the local file can delete what the project file declares. To turn a service off, set its autostart to false as above. To empty a list, such as a service's tags, set it to an empty list.

## Apply the change

Each lane reads the local file in its own worktree. A new worktree has none, since Git does not copy ignored files, so its lanes read the one in the primary worktree, the worktree of lane 0. One file there reaches each lane of the project whose worktree has none.

An edit to the local file changes nothing that already runs. `devlanes status` shows the lanes whose config changed, and `devlanes reload` applies the change to them. An error in the file is reported with the local file's name and line.

## If one worktree needs other changes

Create a local file in that worktree. Its lanes then read that file instead of the primary worktree's, and the two are not merged, so copy over what you want to keep.

## If you use a second project file

Each project file has its own local file, named after it: `devlanes.prod.yaml` reads `devlanes.prod.local.yaml`.

A project file that extends the main one also picks up your main local file, merged right after the file it belongs to. Your personal changes then reach both projects. [Run dev servers and a production build side by side](dev-and-prod.md) sets up such a pair.

## Related

- [Start a service only on demand](../services/on-demand.md)
- [Share settings and choose which services start](templates-and-profiles.md)
- [Config files and reload](../../concepts/config-and-reload.md)
