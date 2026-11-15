---
title: Config files and reload
description: A lane's config is merged from the project file, its local file and the files it extends; an edit leaves running processes alone until a reload applies it.
kind: concept
---

A lane reads its config from the project file, from the local file next to it and from any files the project file builds on. devlanes merges them into one config and watches them while the lane runs. When you edit one, your running processes keep what they started with, status shows what differs, and you choose when to apply it.

## Files merge in a fixed order

Unless you name a file with `--file`, devlanes loads the nearest `devlanes.yaml` in the folder you are in or one above it. The search stops at the repository root, since a file above it belongs to another project.

In most projects two files take part: `devlanes.yaml` and the local file next to it, `devlanes.local.yaml`, which wins where both set the same thing. A project file can also build on other project files. Each of those comes first, each followed by its own local file, and the file you loaded comes last with its local file, so that local file has the final say.

![In the usual case devlanes.yaml and devlanes.local.yaml merge into one config; with extends, devlanes.yaml, devlanes.local.yaml, devlanes.prod.yaml and devlanes.prod.local.yaml merge in that order, a later file winning. Drift is the difference between what runs and what the config would start, and a reload applies it](config-and-reload.svg)

Mappings merge by key, so a local file that sets one variable of a service keeps the others; lists and single values replace. A later file cannot delete what an earlier one set, but it can keep a service from starting by default, or empty a list. Templates, blocks of service settings that services reuse by naming them, expand after the merge, so a template from the base file sees the overrides that came after it.

## The local file belongs to one developer

The local file holds what you want on your machine only, and you keep it out of Git yourself by adding it to `.gitignore`. It cannot rename the project or change which files the project extends. A different name on one machine would give its lanes other domains and container names than everyone else's, and other projects would no longer find it under the name they use.

A worktree without a local file of its own reads the one in the primary worktree, so your overrides reach each lane of the project without a copy per worktree. [Keep personal changes out of Git](../how-to/projects/local-overrides.md) shows what such a file usually holds.

## An edit leaves running processes alone

The daemon checks each file the last load read, the `.env` that expressions read included, and loads the config again when one changes. It does not stop or restart a [unit](../reference/glossary.md) because of that: each running service or task keeps the command and environment it was started with until you apply the change.

Hostnames are the exception, because they change the proxy and not a process. An edited hostname of a service, of a container or of a server you run yourself is routed as soon as the file loads, and so is a change to which services a box, a machine whose lanes are mirrored to others, keeps hidden from those machines. A route still goes to the port the running unit was started with, and a unit whose environment carries one of those URLs keeps the old value until it restarts.

The new routes wait while a start runs in the lane, until it ends. When the same edit needs a larger block of ports, the old hostnames stay until the lane restarts.

## Drift is the difference status shows

devlanes keeps in memory what each running unit was started with and compares it with what the current config would start. That difference is drift, and status reports it on the lane and on each unit it touches.

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

A unit drifts in one of three ways:

- changed: it runs, and the config would start it differently; status names the fields, and a variable by its name without its value
- removed: it runs, and the config no longer has it
- added: the config would run it, and it has not started yet

The comparison uses the environment the lane holds, which the last start, run or exec in it brought. A command from a shell with other values can therefore make units show as changed, and a reload then restarts them with those values. A value you change only in your shell shows nowhere, because a status sends no environment; [Environment and expressions](environment.md) says which commands bring one in.

## Reload applies the difference

A reload stops the removed units, restarts the changed ones and starts the added ones, and leaves the rest running. Here it restarts the worker alone, which is why it counts one service out of the lane's three.

```console title="~/dev/myapp"
$ devlanes reload --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  worker  http://127.0.0.1:20062
```

A reload restarts the whole lane when the container settings changed since the lane was last brought up, or when the edit needs more ports than the lane's block holds. A lane's block of ports can only move while nothing in the lane runs, so that change cannot be made one unit at a time.

## A file that does not load leaves the lane as it was

A typo, a rebase conflict or a deleted file stops the config from loading. While units run, the lane keeps its hostnames and routes on the last config that loaded, so a half-finished edit does not take running services off the proxy.

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  3/3       1.9%  92.8 MiB  https://myapp.localhost  (config changed; C:\Users\you\dev\myapp\devlanes.yaml is invalid at line 14, column 11: services.api.host: unknown key `host`; did you mean `hosts`?)
```

The lane goes on running and carries the error with the file and the line. Starts, restarts and reloads of the lane refuse with that error until the file loads again. A file that names another project after a branch switch moves the worktree to that project, as [Lanes and projects](lanes-and-projects.md) describes, and the old lane keeps running on its last config until you stop it.

## Related

- [Keep personal changes out of Git](../how-to/projects/local-overrides.md)
- [Run dev servers and a production build side by side](../how-to/projects/dev-and-prod.md)
- [Share settings and choose which services start](../how-to/projects/templates-and-profiles.md)
- [When a service fails](../learn/failures.md)
