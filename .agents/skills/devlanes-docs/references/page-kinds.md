# Page kinds

A skeleton for each kind of page. Copy the shape, not the words.

## Learn

One step of the story of myapp. The reader reads and does not type along. About 400 words outside code.

````markdown
---
title: Every worktree gets a lane
description: A second worktree of myapp starts next to the first, on its own ports and its own hostname.
kind: learn
---

The API of myapp runs from the main branch. You start a checkout feature on a branch of its own, and both have to be up together.

## A second worktree starts as lane 1

The project file is committed, so the new folder has it, and the start is the same command as before.

```console
$ git worktree add ../myapp-checkout -b feat/checkout
$ cd ../myapp-checkout
$ devlanes start
```

The main worktree is lane 0. This one becomes lane 1, and its API answers at `https://api.myapp1.localhost`.

## The lane number is in the hostname

(a short paragraph and one fragment or command)

The next page gives myapp a web app that has to find the API of its own lane.

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Lanes and projects](../concepts/lanes-and-projects.md)
````

- Three to five sections, each a heading that states its point, a short paragraph and one fragment or command.
- After a command, one sentence says what happened: which lane, which hostname.
- Show the lines a page adds to the project file, not the whole file.
- The last sentence before the links leads into the next page.

## How-to guide

One goal. About 300 words outside code.

````markdown
---
title: Share one database server between lanes
description: Run one Postgres container for every lane and give each lane its own database in it.
kind: how-to
---

Use this when every lane should have a database of its own and one Postgres is enough for all of them.

::example{slug="infra-postgres-shared"}

## Declare the container

(one sentence, then the smallest config that works)

## Create a database for each lane

(one sentence, then the fragment)

## Start the lane

```console
$ devlanes start
```

## If you pulled a newer image

(a sentence or two and a command)

## Related

- [Remove lanes](../lanes/clean-up.md)
````

- The title starts with a verb and names the goal.
- Steps in the order a person does them. Variations last, as "If ..." headings, two at most.
- One clause of reason at most, then a link to the concept page. Keys are not described one by one: link the config reference.

## Concept

One part of devlanes as a model. About 600 words.

````markdown
---
title: Ports
description: How devlanes gives each lane a block of ports and each service a place in it.
kind: concept
---

No two lanes can listen on the same port, so devlanes hands the ports out. You never write one in the project file.

## Each lane takes a block of ports

(two or three short paragraphs)

![The port pool with the blocks of lane 0 and lane 1 of myapp](ports.svg)

## A service has the same place in every block

(...)
````

- Headings state the point of their section.
- The ordinary case, and the one or two consequences worth knowing. No list of conditions, no exceptions catalogue, no timings.
- A reason only when the code or the owner states one.

## Reference

What a person sets, types or sees.

````markdown
---
title: States and exit codes
description: The states of a unit and a lane, and the exit codes of the devlanes command.
kind: reference
---

## Unit states

| State | Meaning |
|---|---|
| `starting` | The process runs and its ready check has not passed yet. |
| `ready` | The ready check passed. |
````

- The same layout for every entry, in the order of the product.
- No advice and no reasons.
- Complete for what it covers, and it covers only what someone would look up.
