---
title: devlanes
description: devlanes runs every Git worktree of a project at once, each with its own ports, hostnames and data.
kind: concept
---

devlanes runs your project once for every Git worktree you have. Each copy gets its own ports, its own HTTPS hostnames and its own data, so the main branch, a feature branch and the branch your coding agent works on all stay up side by side. The copies can also run on another machine and still open in your browser as if they were local.

The quickest way to try it is to hand it to your coding agent.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then set up devlanes for this repository, start it and give me the URL of each service.
```

To do it by hand, start with [Install](install.md).

## Each worktree runs on its own

A project here has two worktrees. Both run, and each has a URL of its own.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           running  1/1       0.0%  37.2 MiB  https://myapp.localhost
  1     feat/checkout  running  1/1       0.0%  38.2 MiB  https://myapp1.localhost
```

Nothing in the project hardcodes a port or a hostname. devlanes hands them out, so a third worktree needs no planning.

## The parts

![Your machine runs a daemon with a proxy and two lanes of myapp; a box runs a third lane, joined to your machine by one connection](overview.svg)

- A **project** is what one `devlanes.yaml` describes: the services, the containers and the tasks of your app.
- A **lane** is one running copy of a project. Every worktree gets one, numbered from 0.
- A **unit** is one thing a lane runs: a service, a task or a container.
- The **daemon** runs in the background and owns every process, so closing a terminal stops nothing.
- The **proxy** is part of the daemon. It serves every hostname over HTTPS and sends each request to the right lane.
- A **box** is another machine that runs lanes. Your machine shows them under the same hostnames and ports.

The [glossary](reference/glossary.md) has the rest of the words.

## Starting points

- To see how it fits together, read [Learn](learn/first-service.md) from the first page. It follows one project from a single service to lanes on a box.
- For a hostname and HTTPS in front of a server you already run, see [Give a server devlanes does not run a hostname](how-to/services/external.md).
- To put a project of yours on devlanes, see [Add devlanes to a project](how-to/projects/add-devlanes.md), or [Migrate from another process manager](how-to/projects/migrate.md) if mprocs, a Procfile or Compose runs it today.
- To let a coding agent start and test the app, see [Set up a coding agent](how-to/agents/set-up.md).
- To run the lanes on a server, a container or a codespace, see [Prepare a box](how-to/remote/prepare-a-box.md).
