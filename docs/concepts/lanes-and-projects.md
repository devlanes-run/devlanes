---
title: Lanes and projects
description: Each Git worktree of a project runs as a numbered lane with its own hostnames, ports and data, and the project is known by the name in its project file.
kind: concept
---

You work on two branches at once, each in a Git worktree of its own. devlanes runs a lane for each worktree: a running copy of the project with a number, hostnames, a block of ports and a data folder of its own. The project is what one project file describes, and its name together with the lane number gives each lane its hostnames.

## Each worktree runs its own lane

A worktree is a folder with one branch of its own. devlanes ties each lane to one worktree, and knows the worktree by Git's record of it rather than by its path. When you move a worktree with `git worktree move`, its lane keeps its number and its ports.

What a lane runs are its units, chiefly the services and tasks of the project file, next to the containers the project declares, such as a database ([Glossary](../reference/glossary.md)). A start can pick a profile, a named selection of services, and runs the setup entries, such as creating the lane's database, once its containers are up ([Start order and readiness](start-order.md)).

A folder outside Git has one lane. A folder can also get a second lane, which shares every file in it ([Run a folder as two lanes](../how-to/lanes/two-lanes-one-folder.md)).

## A worktree gets its lane when you first use it

The worktree on the primary branch is lane 0, the main lane. The primary branch is the remote's default branch, or main or master without a remote, unless the project file names another.

```yaml devlanes
lanes:
  primary: main
```

That worktree gets its lane without a command running in it. So does the main checkout, the folder the repository was cloned into, when no worktree is on the primary branch, though it then takes the lowest free number. Any other worktree gets its lane the first time you run a command there that acts on the lane or shows its ports, URLs or environment, such as `devlanes start` or `devlanes status`. Reading logs only reads what is there, so it gives a worktree no lane.

A new lane takes the lowest number no other lane holds. Creating a worktree with Git does nothing by itself, and [`lanes.new_worktrees`](/docs/reference/config/project/lanes/) says whether a lane is only reserved, or also set up or started, when the worktree gets it.

CI usually checks a pull request out detached from any branch, so there the main checkout takes lane 0 while it is free, whatever its branch.

## A lane is named after its number

Lane 0 is named main and the others lane1, lane2 and so on, whatever their branch. A value built from the name follows the number.

```yaml devlanes
lanes:
  vars:
    db_name: myapp_${{ lane.name }}
```

That gives myapp_main in lane 0 and myapp_lane1 in lane 1.

## Lane 0 can move to another worktree

Say you switch the folder you cloned into to a feature branch, and check the main branch out in your second worktree. At that worktree's next start, once neither lane runs anything, it becomes lane 0, and the old main lane takes the lowest free number.

![Before the branches swap, ~/dev/myapp is lane 0 and ~/dev/myapp-checkout lane 1; after a start, ~/dev/myapp-checkout is lane 0 and ~/dev/myapp lane 2, each lane keeping its ports and data folder](lanes-and-projects.svg)

Each lane keeps its ports and its data folder, which is keyed by an id that does not change. Its name and domain follow the new number, and devlanes copies and renames nothing. So a database named after the lane changes under it: the new lane 0 opens myapp_main, which the old main lane filled, and the old main lane looks for one under its new name. Data that has to stay with a lane belongs in its data folder ([Files and folders](../reference/files.md#lane-data-folder)).

## A deleted worktree leaves its lane behind

When you delete a worktree, say with `git worktree remove` after a merge, the daemon notices and stops what the lane runs, skipping its `lane.stop` tasks, which need the folder. It keeps the lane itself: its number, port block and data folder, and the containers that belong to the lane alone, which keep running. Status marks the lane as missing its worktree, and a new worktree takes the lowest number still free.

`devlanes lane gc` cleans the lanes whose worktree is gone, while `devlanes lane clean` cleans one lane you name.

A project can have the daemon clean these lanes by itself.

```yaml devlanes
lanes:
  removed_worktrees: clean
```

A running lane is then cleaned at once, and one that runs nothing once its folder has been gone a few seconds, so a moved worktree keeps its lane. The clean removes the lane's own containers and its data folder and frees its number and port block for the next worktree. [Remove lanes](../how-to/lanes/clean-up.md) says what a clean skips without the folder.

## A project is known by its name

The name builds each lane's domain, the names of its containers and lane references such as `myapp@1`, and other projects use it to read this one's URLs. A name belongs to one repository, and two clones of one repository are two repositories. Commands in the second clone are refused while the first is still on disk, and the error asks you to give the second another name. Once the first clone is gone, the name moves to the second.

If you rename the project, or check out a branch whose project file uses another name, the worktree's next command goes to a different project, with new lanes, ports and domains. The old lane stays and keeps running until you stop it. Switching back finds it as it was, unless `devlanes lane gc` cleaned it meanwhile.

## One repository can hold several projects

A command uses the nearest project file above the current folder, up to the root of the repository, since a file above the repository belongs to some other project ([Config files and reload](config-and-reload.md)). An app in a monorepo can therefore be a project of its own, with a project file in its folder and a name of its own.

```yaml devlanes
# apps/billing/devlanes.yaml
project: billing
```

A worktree then holds a lane of each project, such as myapp lane 1 at `myapp1.localhost` and billing lane 1 at `billing1.localhost`, while the worktree's files stay shared. [Split a monorepo into projects](../how-to/projects/monorepo.md) weighs one project against several, and [Run dev servers and a production build side by side](../how-to/projects/dev-and-prod.md) adds a second project file to one folder.

## Related

- [Every worktree gets a lane](../learn/worktrees.md)
- [Remove lanes](../how-to/lanes/clean-up.md)
- [Ports](ports.md)
- [Glossary](../reference/glossary.md)
