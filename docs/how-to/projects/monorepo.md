---
title: Split a monorepo into projects
description: Run a repository of several apps as one project, or give an app a project of its own and let the two read each other's URLs.
kind: how-to
---

Use this guide when one repository holds several apps and you have to decide whether devlanes sees one project or one per app.

::example{slug="workflows-pnpm-monorepo"}

## One project for the repository

A project file at the root, with a service per app, starts every app in one lane under one lane domain. That suits apps you always run together.

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

## A project of its own for an app

A project file in an app's folder makes that app a project with its own name, lanes, ports and domain. Relative paths in it start at that folder, and the root file leaves the app out.

```yaml devlanes
project: billing

services:
  web:
    run_script:
      file: package.json
      name: dev
    hosts: "${{ lane.domain }}"
```

A command uses the nearest project file at or above the folder you run it in. Start billing from its folder, and reach it from anywhere else with `-p`:

```console
$ cd apps/billing
$ devlanes start --no-ui
$ cd ../..
$ devlanes -p billing status
```

## Read another project's URLs

A project reads another project's services through an expression with that project's name. The default covers the time when no lane of billing runs.

```yaml devlanes
global_env:
  BILLING_URL: ${{ projects.billing.services.web.public_url | default('') }}
```

The value is set when a service starts. After billing's lane comes up, a reload in the root project restarts the services whose value changed:

```console
$ devlanes reload
```

## If you want only some apps to start

Keep one project and choose which apps a start brings up with profiles, named choices of services for a start, as [Share settings and choose which services start](templates-and-profiles.md) shows.

## Related

- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [Run scripts your project already has](run-scripts.md)
- [Expressions](../../reference/expressions.md)
- [`project`](/docs/reference/config/project/project/#project)
