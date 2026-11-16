---
title: devlanes
description: devlanes runs your project once per Git worktree, each on its own ports and HTTPS hostnames.
kind: overview
sidebar:
  label: Overview
---

devlanes runs your dev stack once per Git worktree, so two branches of the same project can be up at the same time. Each worktree gets a lane: a numbered place with ports, HTTPS hostnames and data of its own, which it keeps while it is stopped. A lane can have a database of its own as well. Your project needs one small file, and each service has to listen on the port devlanes hands it.

Your coding agent can set it up for you.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then install devlanes on this machine if it is missing, set it up for this repository, start it and give me the URL of each service.
```

The agent installs the devlanes command, leaves the HTTPS setup for you to confirm when your system asks, adds `devlanes.yaml` to the repository and commits nothing unless you agree. To do it by hand, start with [Install devlanes](install.md).

## Two branches, both running

Here the main branch and a feature branch of myapp run side by side, each from its own folder.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           running  2/2       0.0%  85.0 MiB  https://myapp.localhost
  1     feat/checkout  running  2/2       0.0%  84.8 MiB  https://myapp1.localhost
```

Each row is a lane; its URL is the lane's domain, where myapp's web app answers. The API gets a hostname under it, `api.myapp1.localhost` in lane 1. devlanes picked the ports and built the hostnames from the lane's number; the project file names no port and no lane.

## A daemon and a proxy do the work

![A browser on your machine asks for api.myapp1.localhost; the proxy inside the daemon passes the request to the api of lane 1 of myapp, next to lane 0](overview.svg)

A daemon in the background runs the processes of each lane, so they do not depend on the terminal that started them. Its proxy answers each hostname over HTTPS, with certificates your machine trusts once the steps in [Install devlanes](install.md) have run, and passes the request to the lane that hostname belongs to.

The [Glossary](reference/glossary.md) defines the words these docs use.

## Starting points

- To see how the pieces fit, read Learn from its first page, [A service, a port and a hostname](learn/first-service.md). It follows one project from a single service to lanes on another machine.
- For a hostname and HTTPS in front of a server you already run, see [Give a server you run yourself a hostname](how-to/services/external.md).
- To put your own project on devlanes, see [Add devlanes to a project](how-to/projects/add-devlanes.md), or [Migrate from another process manager](how-to/projects/migrate.md) when mprocs, a Procfile or Compose runs it today.
- To let a coding agent start and test the project, see [Set up a coding agent](how-to/agents/set-up.md).
- To run lanes on another machine, such as a VPS, a container or a codespace, and open them in your own browser, see [Prepare a box](how-to/remote/prepare-a-box.md).
