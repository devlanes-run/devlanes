---
title: Config files and reload
description: A lane's config is merged from the project file, its local file and the files it extends; an edit changes nothing that runs until a reload applies it.
kind: concept
---

A lane reads its config from the project file, from the local file next to it and from any files the project file builds on. devlanes merges them into one config, and watches them while the lane runs. An edit changes what the next start would do, but leaves running processes as they are until you apply it.

## Files merge in a fixed order

A project file can name other project files to build on. Each of those comes first, followed by its own local file, then the project file and its local file, so the file you load last has the final say. The project name always comes from the file you load, which lets a production file extend the development one under a project name of its own.

![devlanes.yaml, devlanes.local.yaml, devlanes.prod.yaml and devlanes.prod.local.yaml merge in that order into one config, and a later file wins](config-and-reload.svg)

Mappings merge by key at each level, so a local file that sets one variable of a service keeps the others. Lists and single values replace, and a later file that switches a service from a plain command to a script drops the command. A later file cannot delete what an earlier one set; it turns a service off or empties a list instead. Templates expand after the merge, so a template from the base file sees the overrides that came after it.

## The local file belongs to one developer

The local file holds what one person wants on their machine and stays out of Git. It cannot rename the project or change which files the project extends. A different name on one machine would give its lanes other domains and container names than everyone else's, and other projects would no longer find it under the name they use.

A new worktree has no local file of its own, so its lanes read the one in the primary worktree. One developer's overrides then reach each lane of the project without a copy per worktree. [Keep personal changes out of Git](../how-to/projects/local-overrides.md) shows what such a file usually holds.

## An edit changes nothing that runs

The daemon checks each file the last load read, the `.env` that expressions read included, and loads the config again when one changes. It does not stop or restart a unit because of that. Each running unit keeps the command and environment it was started with, and the next start, restart or reload is when new values take effect.

Hostnames, external ports and what a box may share are the exception. They change only the proxy's routes and what is mirrored, not a process, so they apply as soon as the file loads.

## Drift is the difference status shows

devlanes keeps in memory what each running unit was started with, and compares it with what the current config would start. The result is drift, and status reports it on the lane and on each unit it touches.

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  3/3       0.0%  92.7 MiB  https://myapp.localhost  (config changed: 1 changed; devlanes reload --lane myapp@0 applies it)

  UNIT    STATE  PORT   CPU   MEM       URL
  api     ready  20060  0.0%  36.9 MiB  https://api.myapp.localhost
  web     ready  20061  0.0%  43.1 MiB  https://myapp.localhost
  worker  ready  20062  0.0%  12.7 MiB  http://127.0.0.1:20062
  worker: env API_URL changed since it started
```

A unit can be changed, with the fields that differ, removed from the file while it still runs, or added to the file and not yet started. A variable is named without its value. The record stays in memory because it holds environment values, and devlanes does not write those to disk. A value read from your shell counts too, so a start from a shell where it differs shows as drift.

## Reload applies the difference

A reload stops the units the file removed, restarts the ones it changed and starts the ones it added. Here only the worker restarts, and the API and the web app keep their processes.

```console title="~/dev/myapp"
$ devlanes reload --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  worker  http://127.0.0.1:20062
```

A reload restarts the whole lane in two cases: the container settings changed since the lane's last whole start, or the edit needs more ports than the lane's block holds. Containers come up before services, and a lane's port block can only move while nothing in the lane runs, so neither change can be applied to one unit. A reload keeps the environment the lane's last start brought, so it does not matter which shell runs it. Restarting a single unit also picks up the current config, without a reload.

## A file that does not load leaves the lane as it was

A typo, a rebase conflict or a deleted file stops the config from loading. While units run, the lane keeps its hostnames and routes on the last config that loaded, so a half-finished edit does not take running services off the proxy. A project file that suddenly names another project counts the same way.

Status shows the error on the lane's row, with the file, the line and the key, and the lane goes on running. Starts, restarts and reloads refuse with that error until the file loads again, since devlanes has no valid config to start from.

## Related

- [Keep personal changes out of Git](../how-to/projects/local-overrides.md)
- [Run dev servers and a production build side by side](../how-to/projects/dev-and-prod.md)
- [Share settings and choose which services start](../how-to/projects/templates-and-profiles.md)
- [When a service fails](../learn/failures.md)
