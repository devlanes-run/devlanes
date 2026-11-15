---
title: Lanes and projects
description: A project is what one project file describes, and each Git worktree that runs it becomes a numbered lane with its own name.
kind: concept
---

A project is what one project file describes: a name and the services, tasks and containers that make it up. A lane is one running copy of that project, tied to one Git worktree. The lane's number and the project's name are what devlanes builds ports, hostnames and container names from, so the project file itself names none of them.

## Each worktree runs its own lane

A worktree is a folder with its own checkout of a branch, so lanes in two worktrees share no source files, build output or installed dependencies. devlanes ties each lane to a worktree for that reason, and recognises the worktree by Git's own record of it rather than by its path. Moving a worktree with Git's move command keeps its lane, with the same number and ports.

![Two worktrees of one repository, on main and feat/checkout; each holds a lane of myapp and a lane of billing](lanes-and-projects.svg)

A folder outside Git has one lane. A second lane in the same folder is possible, as [Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md) shows, but the two then share every file in it, so a lane is only as separate as its worktree.

## Lane 0 follows the primary branch

The worktree on the primary branch is lane 0, the main lane. By default that branch is the remote's default branch, or `main` or `master` when there is no remote; the project file can name another.

```yaml devlanes
lanes:
  primary: main
```

The main lane is the only one that exists before anything runs in it. Any other worktree gets its lane the first time a command that acts on lanes runs there, such as a start or a status, and takes the lowest free number. Reading logs does not count, so a worktree you only look into stays without ports. [`lanes.new_worktrees`](/docs/reference/config/project/lanes/) decides whether a new lane is also set up or started.

The number can move. When another worktree checks out the primary branch, it takes lane 0 at its next start and the old main lane gets a new number. The lane's data folder is keyed by an id that does not change, so its files stay with it. In CI the checkout takes lane 0 on whatever branch it is on, because CI checks pull requests out detached from any branch.

## A lane is named after its number

Lane 0 is named main and the others lane1, lane2 and so on. The name follows the number and not the branch, so a value built from it fits where a branch name with a slash would not, such as a database name.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}
```

This gives myapp_main in lane 0 and myapp_lane1 in lane 1. The branch is still there for display and for commands: a lane can be addressed as myapp@1 or by its branch, as [Lane and unit references](../reference/lane-and-unit-references.md) lists.

## A project is the name in its file

devlanes knows a project by its name alone. The name builds each lane's domain and the names of the containers devlanes creates, so two repositories with one name would claim the same hostnames. A name therefore belongs to one repository, and a second repository that uses it fails to start while the first is still on disk.

Renaming a project makes a new project, with new lanes, ports and domains. The old project's lanes stay, and what they run keeps running until you stop it. A branch whose project file names another project moves its worktree to that project the same way, and switching back finds the old lane as it was. [Remove lanes](../how-to/lanes/clean-up.md) cleans up the lanes left behind.

## One repository can hold several projects

A command uses the nearest project file above the current folder, up to the root of the repository. A file above the repository belongs to some other project, so the search stops there. A monorepo can give an app a project of its own with a project file in that app's folder.

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

The two projects have their own lanes, port blocks and domains, and a worktree holds a lane of each, as in the diagram above. Paths in the billing file start in its own folder. The files of the worktree are still shared, so build output one project writes there is the other's too. [Split a monorepo into projects](../how-to/projects/monorepo.md) weighs one project against several.

## A second project file runs the same code another way

A project file other than the nearest one is named once with the global `--file` flag, and after that the project's name finds it. The usual case is a production build next to the dev servers, in a file that extends the first and sets a name of its own.

```yaml devlanes
# devlanes.prod.yaml
extends: devlanes.yaml
project: myapp-prod
```

Because the name differs, myapp-prod is a separate project: lane 0 of it answers on its own domain next to myapp's, with its own ports, from the same worktree. [Config files and reload](config-and-reload.md) explains how the two files merge, and [Run dev servers and a production build side by side](../how-to/projects/dev-and-prod.md) sets one up.

## Related

- [Every worktree gets a lane](../learn/worktrees.md)
- [Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Ports](ports.md)
