---
title: Split a monorepo into projects
description: Run a repository of several apps as one project, or give an app a project of its own and let the two read each other's URLs.
kind: how-to
---

Use this guide when one repository holds several apps and you have to decide how many projects devlanes sees in it. You end up with each app on its own hostnames, started together or each on its own.

::example{slug="workflows-pnpm-monorepo"}

## Choose one project or one per app

One project file at the root, with a service per app, starts every app in one lane and puts them all under one lane domain. That suits apps you always run together. An app with a project file of its own gets its own name, lanes, ports and domain, and you start it apart from the rest.

```yaml devlanes
project: myapp

services:
  api:
    run_script:
      file: apps/api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
  web:
    preset: vite
    run_script:
      file: apps/web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
```

Each service points at its own app's package.json, so each script runs in its app's folder. Leave an app you split off out of the root file.

## Give an app its own project file

Put a project file in the app's folder with a name of its own. Relative paths in it start at that folder, so the script below is the app's own package.json.

```yaml devlanes
# apps/billing/devlanes.yaml
project: billing

services:
  web:
    run_script:
      file: package.json
      name: dev
    hosts: "${{ lane.domain }}"
    ready:
      http:
        path: /healthz
```

Its services get hostnames under a lane domain built from billing, apart from myapp's. A worktree of the repository can run a lane of each project side by side, and each runs its own tasks when the lane is new, as [Run installs, migrations and seeds](../services/tasks.md) shows.

## Run commands in the right project

A command uses the nearest project file at or above the folder you run it in, up to the repository root. Anywhere under the billing folder that is the billing project, and anywhere else it is the root one.

From another folder, name the project with `-p`, such as `devlanes -p billing status`. That works once a start or a status has run in the project's own folder.

## Read another project's URLs

In billing's project file, a service reaches myapp's services through an expression with that project's name, written with underscores in place of dashes. Give it a default for when myapp has no lane to read.

```yaml devlanes
services:
  web:
    env:
      MYAPP_API_URL: ${{ projects.myapp.services.api.public_url | default('') }}
```

devlanes looks among that project's lanes that run a service for the one on the same branch, else the same lane name, else lane 0, and the default applies when none matches. The value is set when the service starts, so restart it after the other project's lane comes up.

## If the repository root has no project file

A repository root without a project file of its own can hold several projects below it. There, a command that acts on one project fails and tells you to name one with `-p`.

## If one project per app is too many lanes to start

Keep one project and choose which apps a start brings up with profiles, named choices of services for a start, as [Share settings and choose which services start](templates-and-profiles.md) shows.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Run scripts your project already has](run-scripts.md)
- [Expressions](../../reference/expressions.md)
- [`project`](/docs/reference/config/project/project/)
