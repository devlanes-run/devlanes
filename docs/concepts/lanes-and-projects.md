---
title: Lanes and projects
description: A project is what one project file describes, and each Git worktree that runs it becomes a numbered lane with its own ports, hostnames and data.
kind: concept
---

A project is what one project file describes. A lane is one running copy of that project for one Git worktree. Each lane takes its ports, hostnames and container names from its number and the project's name, so the project file writes them once for every lane.

## A lane is made of units and containers

What a lane runs are its units. Services are the long-running processes, such as the API. Tasks are one-off commands at moments in a lane's life, such as an install when the lane is created ([Run installs, migrations and seeds](../how-to/services/tasks.md)). Next to its units a lane has containers, which the project declares under infra, such as a Postgres that the lanes share ([A database for each lane](../learn/databases.md)).

Setup entries run once a lane's containers are up, such as creating that lane's database in the shared Postgres ([Start order and readiness](start-order.md)). A profile is a named selection of services a start can ask for ([Share settings and choose which services start](../how-to/projects/templates-and-profiles.md)). Each lane also has a data folder outside its worktree, for files no other lane may share ([Files and folders](../reference/files.md#lane-data-folder)).

## Each worktree runs its own lane

A worktree is a folder with one branch of its own. devlanes ties each lane to one worktree, and knows the worktree by Git's record of it rather than by its path. When you move a worktree with `git worktree move`, its lane keeps its number and its ports.

A folder outside Git has one lane. You can add a second lane to a folder ([Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md)), and the two then share every file in it.

## Lane 0 follows the primary branch

The worktree on the primary branch is lane 0, the main lane. The primary branch is the remote's default branch, or main or master without a remote, unless the project file names another.

```yaml devlanes
lanes:
  primary: main
```

That worktree is the only one that gets its lane without a command running in it. Another worktree gets its lane the first time you run a command there that acts on a lane or shows its ports and URLs, such as `devlanes start` or `devlanes status`, and it takes the lowest free number. Reading logs does not count.

Creating a worktree with Git does nothing by itself. [`lanes.new_worktrees`](/docs/reference/config/project/lanes/) says what happens at the moment a worktree gets its lane: the lane is only reserved, or also set up, or also started.

In CI the main checkout, the folder the repository was cloned into, takes lane 0 on whatever branch it is on, while lane 0 is free, because CI usually checks a pull request out detached from any branch.

## Lane 0 can move

Say you switch the folder you cloned into to a feature branch and check the main branch out in another worktree. That worktree takes lane 0 at its next start, once neither it nor the lane holding 0 runs anything, and the old holder takes the lowest free number. A lane devlanes renumbers keeps its ports and its data folder. Its name and domain follow the new number, and so does every value built from them, such as a database named after the lane.

## A lane is named after its number

Lane 0 is named main and the others lane1, lane2 and so on, whatever their branch. A value built from the name changes with the number, as a database name does here.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}
```

That gives myapp_main in lane 0 and myapp_lane1 in lane 1. Commands can name a lane by its branch too ([Lane and unit references](../reference/lane-and-unit-references.md)).

## A deleted worktree leaves its lane behind

When you delete a worktree, say with `git worktree remove` after a merge, the daemon notices within seconds and stops what the lane runs. It keeps the lane itself: its number, port block, data folder and its own containers. Status marks it as missing its worktree, and a new worktree takes the next free number instead of reusing that one. `devlanes lane gc` cleans such lanes and frees all of it ([Remove lanes](../how-to/lanes/clean-up.md)).

A project can ask the daemon to clean these lanes by itself.

```yaml devlanes
lanes:
  removed_worktrees: clean
```

The daemon then waits until the folder has been gone a few seconds, so a moved worktree keeps its lane. Tasks and teardown that need the folder are skipped, so data they would drop from a shared container, such as the lane's database in the shared Postgres, stays.

## A project is known by its name

The name builds each lane's domain, the names of its containers, the lane references such as `myapp@1`, and the way another project reaches this one in an expression. A name belongs to one repository. Two clones of the same repository are two repositories: commands in the second clone are refused while the first is still on disk, and once the first is gone the name moves to the second. On one machine, use worktrees of one clone.

Renaming a project makes a new project, with new lanes, ports and domains, and a branch whose file names another project moves its worktree the same way. The old lanes stay, and what they run keeps running until you stop it. Switching back finds the old lane as it was, unless `devlanes lane gc` cleaned it in between.

## One repository can hold several projects

A command uses the nearest project file above the current folder, up to the root of the repository, since a file above the repository belongs to some other project. So an app in a monorepo can be a project of its own, with a project file in its folder.

```yaml devlanes
# apps/billing/devlanes.yaml
project: billing
services:
  api:
    run_script:
      file: package.json
      name: dev
    hosts: "${{ lane.domain }}"
```

Paths in that file start in its own folder. A worktree then holds a lane of each project, each with its own domain, while the worktree's files stay shared.

![Two worktrees of one repository, on main and feat/checkout; each holds a lane of myapp and a lane of billing, each lane with its own domain](lanes-and-projects.svg)

[Split a monorepo into projects](../how-to/projects/monorepo.md) weighs one project against several.

## A second project file runs the same code another way

A production build can run next to the dev servers from a second file that extends the first and sets a name of its own.

```yaml devlanes
# devlanes.prod.yaml
extends: devlanes.yaml
project: myapp-prod
```

myapp-prod is then a separate project, with its own lanes, ports and domain, in the same worktree. devlanes finds only `devlanes.yaml` by itself, so the first command for the other file names it with the global `--file` flag ([Run dev servers and a production build side by side](../how-to/projects/dev-and-prod.md)).

## Related

- [Every worktree gets a lane](../learn/worktrees.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Ports](ports.md)
- [Glossary](../reference/glossary.md)
