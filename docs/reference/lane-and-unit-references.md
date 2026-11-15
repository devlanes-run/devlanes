---
title: Lane and unit references
description: The forms a lane and a unit can be named in on the command line, and how a command picks the project and the lane when none is named.
kind: reference
---

Commands name a lane with a lane reference, as in `--lane 2`, and a unit with a unit reference, as in `devlanes logs 2:api`. The names in them are the ones `devlanes status` prints.

## Lane references

A lane reference is `[project@]lane`. Without `project@`, the lane is looked for in the project of the current directory ([Picking the project](#picking-the-project)).

| Form | Matches | Example |
|---|---|---|
| `N` | The lane with number N. | `1` |
| `branch` | The lane whose worktree has that branch checked out. | `feat/checkout` |
| `name` | The lane's name: `main` for lane 0, `lane<N>` for lane N, or the name given with `devlanes lane new --name`. | `lane1`, `review` |
| `folder` | The lane whose worktree folder has that name. | `myapp-checkout` |
| Absolute path | The lane whose worktree is at that path. | `/home/you/dev/myapp-checkout` |
| `./path` | A path from the current directory. | `./myapp-checkout` |
| `~/path`, `~` | A path from the home folder in `HOME`. | `~/dev/myapp-checkout` |
| `project@lane` | Any of the forms above, in the project `project`. | `myapp@1`, `myapp@feat/checkout` |

On Windows a path may also start with a drive letter (`C:\`, `C:/`), `\\server\share`, `\\?\`, `.\`, `..\` or `~\`, and paths compare without regard to case.

A project name is lowercase letters, digits, `.`, `_` and `-`, and starts with a letter or a digit.

### Matching order

| Step | Rule |
|---|---|
| 1 | A reference made only of digits matches lane numbers and nothing else. |
| 2 | Any other reference is tried as a branch, then a lane name, then a folder name, then a path. The first of these that matches one lane decides. |
| 3 | Two lanes that match at the same step make the reference ambiguous. |
| 4 | A worktree that is not a lane yet is matched by its folder name, and only when no lane matched. |

Branches, lane names and folder names compare exactly, with case. Paths compare without case on Windows and with case elsewhere.

## Unit references

A unit reference is `[lane:]unit`, where `lane` is any lane reference. Without `lane:`, the unit is looked for in the lane the command picks ([Picking the lane](#picking-the-lane)).

| Form | Matches | Example |
|---|---|---|
| `unit` | The unit with that name. | `api` |
| `lane:unit` | The unit in that lane. | `1:api`, `feat/checkout:web` |
| `project@lane:unit` | The unit in that lane of that project. | `myapp@1:api` |
| `service.N` | Replica N of a service with replicas. | `worker.2` |
| `service` | In commands that take several units: every replica of a service with replicas. | `worker` |
| Glob | In commands that take several units: every unit whose name matches the pattern. | `"api-*"` |
| `+tag` | In commands that take several units: every unit with that tag. | `+backend` |

A unit is a service or a replica of one, a task, an `exec` command by its `--name`, an `infra.setup` or `infra.teardown` entry as `setup.<entry>` or `teardown.<entry>`, or an `infra.external` entry. Containers are not units.

A glob takes `*`, `?`, `[abc]`, `[a-z]` and `{a,b}`. A shell expands an unquoted glob before devlanes sees it. A tag is one of the unit's `tags`. Unit names and globs compare with case.

On Windows the colon after a drive letter belongs to the path: `C:\src\myapp:web` is the unit `web` of the lane at `C:\src\myapp`.

### Several units

Commands that take several units, such as `start`, `stop`, `restart` and `logs`, follow these rules.

| Rule | Result |
|---|---|
| Each reference has to match at least one unit. | A reference that matches none fails, and nothing is done. |
| References that name a lane have to name it in the same words. | `1:api myapp@1:web` fails with `mixed_lanes`, though both name lane 1. |
| References without a lane use the lane that the others name. | `start 1:api web` starts `api` and `web` of lane 1. |
| On `start`, a name, glob or tag may select a service with `autostart: false`. | In a profile's list, a glob skips such services; a name or a tag selects them. |
| On `start`, `stop`, `restart` and `reload`, a lone reference without a lane that names no unit but is the number or the name of a lane | Fails and names that lane, with `--lane` as the fix. |

## Picking the project

A command takes the first of these that applies.

| Step | Project |
|---|---|
| 1 | `--file FILE`: the project that file names. |
| 2 | `--project NAME` or `-p NAME`, or `DEVLANES_PROJECT`: that project. |
| 3 | `DEVLANES_LANE_PROJECT`, which devlanes sets in every unit, when the current directory is in one of that project's worktrees. |
| 4 | The nearest `devlanes.yaml` in the current directory or its parents, up to the root of the Git repository. |
| 5 | Without one, the projects whose worktrees hold the current directory. |
| 6 | `DEVLANES_LANE_PROJECT`, when it is set. |
| 7 | None. |

A project from a file other than `devlanes.yaml` is known once a command has named its file with `--file`. After that, `--project` finds it.

When several projects share the folder, `DEVLANES_LANE_PROJECT` picks one of them. Otherwise a command that needs one project fails with `project_ambiguous`.

A lane reference that starts with `project@` uses that project, whatever the current directory.

## Picking the lane

| Step | Lane |
|---|---|
| 1 | The lane in a unit reference, `lane:unit`. |
| 2 | `--lane LANE`. |
| 3 | The lane of the worktree that holds the current directory. In a folder that also has lanes added with `lane new --name`, its first lane. |
| 4 | With `--file`, outside the project's repository: the lane whose worktree holds that file. |
| 5 | With `--project`, `-p`, `DEVLANES_PROJECT` or `--file`, outside the project's repository: lane 0. Without lane 0, the project's only lane. With several lanes and no lane 0 the command fails with `ambiguous`, and with no lane with `not_found`. |

Step 5 does not apply to `run`, `exec`, `lane new`, `lane clean` and `lane gc`. Outside the repository they fail with `lane_required` and need `--lane`. Inside the repository, a directory in none of the project's worktrees fails with `not_found`.

Commands that list lanes, such as `status`, `lane ls` and `stop --mine`, cover more than one lane:

| Given | Lanes covered |
|---|---|
| `--lane` | That lane. |
| `--file` or `--project` | The lanes of that project. |
| A directory of one project | The lanes of that project. |
| A folder that holds several projects, such as a repository root | The lanes of each of them. |
| A directory outside every project, with `DEVLANES_LANE_PROJECT` set | The lanes of that project. |
| A directory outside every project, otherwise | Every lane. |

## A missing reference

Some commands need a unit, a task or a lane and can list the choices: `logs`, `url print`, `url open`, `attach`, `wait`, `debug why`, `run` and `lane clean`.

| Situation | Result |
|---|---|
| Human output on a terminal, outside CI | A picker lists the choices. Cancelling it exits with 130. |
| Any other output | The command fails with exit code 2, lists up to 20 choices and prints the command with the first one filled in. |
| No `devlanes.yaml` in the directory or its parents, or a directory in no lane | Exit code 3. |
| A lane with nothing of that kind | Exit code 2. |

## Errors

| Code | Exit code | When | Data |
|---|---|---|---|
| `invalid_reference` | 2 | The reference is not in one of the forms above, or a glob does not parse. | `reference` |
| `not_found` | 3 | No lane, unit or project matches. | `reference`, `candidates` |
| `ambiguous` | 3 | Several lanes match at the same step. | `reference`, `candidates` |
| `mixed_lanes` | 2 | Unit references name two lanes. | `units` |
| `lane_required` | 2 | A command needs `--lane` outside the project's repository. | `project`, `candidates` |
| `project_ambiguous` | 3 | Several projects share the folder. | `path`, `candidates` |
| `no_config` | 3 | No `devlanes.yaml` in the directory or its parents, up to the repository root. | `path`, `searched_to` |
| `project_not_started` | 3 | The project file names a project no command has started, and the command only reads. | `project`, `file` |
| `argument_missing` | 2 or 3 | A command that needs a reference got none and cannot ask ([A missing reference](#a-missing-reference)). | Listed on [States and exit codes](states-and-exit-codes.md#errors). |

`candidates` holds references that can be pasted into the next command: lanes as `project@N`, units as `project@N:unit`. How an error is printed is on [States and exit codes](states-and-exit-codes.md#errors).

## Lanes in output

Output names a lane as `project@N`, such as `myapp@1`. A lane without a number is `project@name`, or `project@folder`. In JSON the field is `reference`. Each of these is a lane reference, so it can be pasted back into a command unchanged.
