---
title: Keep personal changes out of Git
description: Turn services off, change variables and add services on your machine only, in a local file next to the project file.
kind: how-to
---

Use this guide when you want the project to run a little differently on your machine, such as one service off or a debug log level, without changing the project file the team shares.

::example{slug="workflows-dev-and-prod"}

## Create the local file

The local file is `devlanes.local.yaml`, next to `devlanes.yaml`. It takes the same keys and holds only what you change, merged over the project file:

```yaml devlanes
# devlanes.local.yaml
services:
  web:
    autostart: false
  api:
    env:
      LOG_LEVEL: debug
  scratch:
    run_script:
      file: apps/scratch/package.json
      name: dev
    hosts: "scratch.${{ lane.domain }}"
```

Here a plain start leaves the web service out unless the lane ran it before or you name it, the API gets one more variable and keeps the rest, and a service only you run joins the lane. The local file cannot set `project` or a file-level `extends`. [Config files and reload](../../concepts/config-and-reload.md#files-merge-in-a-fixed-order) has the merge rules.

## Keep it out of Git

devlanes does not touch your `.gitignore`, so add the file yourself:

```text title=".gitignore"
devlanes*.local.yaml
```

## Apply the change

```console title="~/dev/myapp"
$ devlanes reload
```

The reload restarts the services your edit changed. A new worktree has no local file of its own and reads the primary worktree's, as [Files and folders](../../reference/files.md#files-of-a-project) describes.

## If one worktree needs other changes

Create a local file in that worktree. Its lanes read that file instead of the primary worktree's, and the two are not merged.

## If you use a second project file

Each project file has its own local file, named after it: `devlanes.prod.yaml` reads `devlanes.prod.local.yaml`. [Run dev servers and a production build side by side](dev-and-prod.md) sets up such a pair.

## Related

- [Start a service only on demand](../services/on-demand.md)
- [Give a server you run yourself a hostname](../services/external.md)
- [Config files and reload](../../concepts/config-and-reload.md)
