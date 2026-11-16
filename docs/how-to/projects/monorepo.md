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
  admin:
    preset: vite
    run_script:
      file: apps/admin/package.json
      name: dev
    hosts: "admin.${{ lane.domain }}"
```

Each service points at its own app's package.json, so each script runs in its app's folder. Leave an app you split off out of the root file. A start at the root brings up the root project's services:

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  3 of 3 ready
  ✓  api    https://api.myapp.localhost
  ✓  web    https://myapp.localhost
  ✓  admin  https://admin.myapp.localhost
```

## Give an app its own project file

Put a project file in the app's folder with a name of its own. Relative paths in it start at that folder, so the script below is the app's own package.json.

```yaml devlanes
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

Start it from the app's folder. Its services get hostnames under a lane domain built from billing, apart from myapp's, and its lane runs next to the root project's in the same worktree.

```console title="~/dev/myapp/apps/billing"
$ devlanes start --no-ui
✓ services ready
billing@0 · main
✓ services  1 of 1 ready
  ✓  web  https://billing.localhost
```

Each project runs its own tasks when its lane is new, as [Run installs, migrations and seeds](../services/tasks.md) shows.

## Run commands in the right project

A command uses the nearest project file at or above the folder you run it in, up to the repository root. Anywhere under the billing folder that is the billing project, and anywhere else it is the root one, so a plain status at the root lists only myapp.

From another folder, name the project with `-p`. That works once a start or a status has run in the project's own folder.

```console title="~/dev/myapp"
$ devlanes -p billing status
billing
  LANE  BRANCH  STATE    SERVICES  CPU   MEM        URL
  0     main    running  1/1       0.0%  177.5 MiB  https://billing.localhost

  UNIT  STATE  PORT   CPU   MEM        URL
  web   ready  20080  0.0%  177.5 MiB  https://billing.localhost
```

## Read another project's URLs

In the root project file, a value reaches billing's services through an expression with that project's name, written with underscores in place of dashes. Give it a default for when billing has no lane to read.

```yaml devlanes
global_env:
  VITE_BILLING_URL: ${{ projects.billing.services.web.public_url | default('') }}
```

devlanes looks among that project's lanes that run a service for the one on the same branch, else the same lane name, else lane 0, and the default applies when none matches. The value is set when a service starts. Once billing's lane comes up, status marks the root project's services whose value changed:

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM        URL
  0     main    running  3/3       9.7%  474.0 MiB  https://myapp.localhost  (config changed: 3 changed; devlanes reload --lane myapp@0 applies it)

  UNIT   STATE  PORT   CPU   MEM        URL
  api    ready  20071  2.5%  122.4 MiB  https://api.myapp.localhost
  web    ready  20072  2.8%  174.7 MiB  https://myapp.localhost
  admin  ready  20070  4.4%  177.0 MiB  https://admin.myapp.localhost
  api: env VITE_BILLING_URL changed since it started
  web: env VITE_BILLING_URL changed since it started
  admin: env VITE_BILLING_URL changed since it started
```

A reload restarts those services with the new value:

```console title="~/dev/myapp"
$ devlanes reload
```

## If the repository root has no project file

A repository root without a project file of its own can hold several projects below it. There, a command that acts on one project fails and tells you to name one with `-p`.

## If one project per app is too many lanes to start

Keep one project and choose which apps a start brings up with profiles, named choices of services for a start, as [Share settings and choose which services start](templates-and-profiles.md) shows.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Run scripts your project already has](run-scripts.md)
- [Expressions](../../reference/expressions.md)
- [`project`](/docs/reference/config/project/project/#project)
