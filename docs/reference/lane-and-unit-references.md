---
title: Lane and unit references
description: The forms a lane and a unit can be named in on the command line, and how a command picks the project and the lane when none is named.
kind: reference
---

Commands name a lane with a lane reference, as in `--lane 1`, and a unit with a unit reference, as in `devlanes logs 1:api`. The names in them are the ones `devlanes status` prints.

## Lane references

A lane reference is `[project@]lane`. Without `project@`, the lane is looked for in the project of the current directory.

| Form | Matches | Example |
|---|---|---|
| `N` | The lane with number N. | `1` |
| `branch` | The lane whose worktree has that branch checked out. | `feat/checkout` |
| `name` | The lane's name: `main`, `lane<N>`, or a name given with `devlanes lane new --name`. | `review` |
| `folder` | The lane whose worktree folder has that name. | `myapp-checkout` |
| A path | The lane whose worktree is at that path. | `~/dev/myapp-checkout` |
| `project@lane` | Any of the forms above, in the project `project`. | `myapp@1` |

A reference made only of digits matches lane numbers only. Any other reference is tried as a branch, then a lane name, then a folder name, then a path.

## Unit references

A unit reference is `[lane:]unit`, where `lane` is any lane reference. Without `lane:`, the unit is looked for in the lane of the current directory, or in the one `--lane` names.

| Form | Matches | Example |
|---|---|---|
| `unit` | The unit with that name. | `api` |
| `lane:unit` | The unit in that lane. | `1:api` |
| `project@lane:unit` | The unit in that lane of that project. | `myapp@1:api` |
| `service.N` | Replica N of a service with replicas. | `worker.2` |

Commands that take several units, such as `start`, `stop`, `restart` and `logs`, also take these:

| Form | Matches | Example |
|---|---|---|
| `service` | Every replica of a service with replicas. | `worker` |
| A glob | Every unit whose name matches. | `"api-*"` |
| `+tag` | Every unit with that tag. | `+backend` |

A shell expands a glob without quotes before devlanes sees it.

## Picking the project

| When you give | The project is |
|---|---|
| `--file FILE` | The one that file names. |
| `--project NAME`, `-p NAME`, or `DEVLANES_PROJECT` | That one. |
| Nothing | The one of the nearest `devlanes.yaml` in the current directory or its parents, up to the root of the Git repository. |

A project from another file, such as `devlanes.prod.yaml`, is known once a command has named it with `--file`; after that, `--project` finds it. Inside a unit, devlanes commands use the unit's project.

## Picking the lane

| When you give | The lane is |
|---|---|
| A unit reference with `lane:` | That lane. |
| `--lane LANE` | That lane. |
| Nothing, inside a worktree | The lane of that worktree. |
| `--project` or `--file`, outside the repository | Lane 0. `run`, `exec`, `lane new` and `lane clean` need `--lane` there. |

## When a reference matches nothing

A reference that matches nothing, or several lanes, fails with exit code 3 and lists the references that do exist. Output names lanes as `myapp@1`, which you can paste back into a command.
