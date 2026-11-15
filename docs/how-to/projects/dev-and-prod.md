---
title: Run dev servers and a production build side by side
description: Add a second project file that runs the same services from a production build, next to the dev servers of the same worktree.
kind: how-to
---

Use this guide when you want the built app running next to the dev servers, from the same worktree, to compare the two or to test the build. A second project file runs the same services as a project of its own, with its own ports and hostnames.

::example{slug="workflows-dev-and-prod"}

## Give each app a start script

The second project runs each app from its build, so each package.json that holds a dev script needs a script that serves the build. In myapp the web app gets a build and a start script, and the API gets a start script that runs the server without the watch flag.

```json
{
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "start": "vite preview"
  }
}
```

A start fails on a service whose file has no start script, and lists the scripts the file has. `devlanes config check` does not open the scripts, so it passes such a file.

## Write the second project file

Put the file next to `devlanes.yaml`, here the one from [Services that find each other](../../learn/connecting-services.md) with its install task. It extends that file, so it has the same services and tasks, and it sets its own project name, so its lanes, ports and hostnames are its own. The way the files merge is in [Config files and reload](../../concepts/config-and-reload.md).

```yaml devlanes
# devlanes.prod.yaml
extends: devlanes.yaml
project: myapp-prod

global_env:
  NODE_ENV: production

tasks:
  install:
    # global_env reaches tasks too, and npm skips devDependencies such as Vite under production.
    env:
      NODE_ENV: development
  build:
    run_script:
      file: web/package.json
      name: build
    when: lane.start

profiles:
  default:
    # Without include here or in devlanes.yaml, this profile would start no services.
    include: ["*"]
    run_scripts:
      dev: start
```

The default profile, the services a lane's first plain start brings up, runs `start` wherever a service's script is `dev`. The build task runs before the services on every start, and the install task keeps the development dependencies the build needs.

Any file name works except one that ends in `.local.yaml` or `.local.yml`.

## Check and start it with --file

devlanes finds only `devlanes.yaml` by itself, so name the second file with the global `--file` flag the first time.

```console title="on Windows 11"
$ devlanes --file devlanes.prod.yaml config check
ok myapp-prod (C:\Users\you\dev\myapp\devlanes.prod.yaml)
```

Start it with `devlanes --file devlanes.prod.yaml start`. After that first start, `-p myapp-prod` reaches the project from any folder, and plain commands in the worktree keep using the dev project.

The second project numbers its lanes on its own. In the main worktree the built web app answers on `https://myapp-prod.localhost`, next to the dev server on `https://myapp.localhost`.

## If both projects build into the same folder

Build output in the worktree, such as a dist or .next folder, is one folder for every lane that runs from that worktree, in either project. Point it at the lane's data folder, a folder devlanes keeps for each lane outside the worktree, and have the build read the path from the variable.

```yaml devlanes
global_env:
  WEB_OUT_DIR: ${{ lane.data_path }}/web-dist
```

## If both projects use one database

A value built only from the lane's name is the same in both projects, so both would use one database. Put the project's name in it.

```yaml devlanes
lanes:
  vars:
    db_name: ${{ project.name | replace('-', '_') }}_${{ lane.name }}
```

Shared Compose containers are one set for both projects, so stopping them from one project stops them for the other. [Share one database server between lanes](../infra/shared-database.md) sets up the database.

## If only you need changes to the second project

Its local file is named after it, `devlanes.prod.local.yaml`. [Keep personal changes out of Git](local-overrides.md) covers what goes there.

## Related

- [Share settings and choose which services start](templates-and-profiles.md)
- [Lanes and projects](../../concepts/lanes-and-projects.md)
- [`extends`](/docs/reference/config/project/extends/)
- [`profiles.<name>.run_scripts`](/docs/reference/config/project/profiles/)
