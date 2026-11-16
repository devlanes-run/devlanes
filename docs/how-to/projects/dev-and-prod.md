---
title: Run dev servers and a production build side by side
description: Add a second project file that runs the same services from a production build, next to the dev servers of the same worktree.
kind: how-to
---

Use this guide when you want the built app running next to the dev servers of the same worktree, with its own ports and hostnames.

::example{slug="workflows-dev-and-prod"}

## Give each app a start script

The second project runs each app from its build, so each package.json with a dev script also needs a script that serves the build. The API's start script runs the server without the watch flag.

```json title="web/package.json"
{
  "scripts": {
    "dev": "vite",
    "build": "vite build",
    "start": "vite preview"
  }
}
```

## Write the second project file

Put it next to `devlanes.yaml`, here the one from [Services that find each other](../../learn/connecting-services.md). It extends that file, so it has the same services and tasks, and its own project name gives it its own lanes, ports and hostnames.

```yaml devlanes
# devlanes.prod.yaml
extends: devlanes.yaml
project: myapp-prod

global_env:
  NODE_ENV: production

tasks:
  install:
    # npm skips devDependencies such as Vite under production, and the build needs them.
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

The default profile, the services a lane's first start brings up, runs `start` wherever a service's script is `dev`, and the build runs before the services on every start.

## Start it

devlanes finds only `devlanes.yaml` by itself, so name the second file the first time. After that, `-p` reaches the project.

```console
$ devlanes --file devlanes.prod.yaml start --no-ui
$ devlanes -p myapp-prod status
```

The built app answers on `https://myapp-prod.localhost`, next to the dev server on `https://myapp.localhost`.

## If both projects use one database

A value built only from the lane's name is the same in both projects. Put the project's name in it to keep their data apart:

```yaml devlanes
lanes:
  vars:
    db_name: ${{ project.name | replace('-', '_') }}_${{ lane.name }}
```

## Related

- [Config files and reload](../../concepts/config-and-reload.md)
- [Share settings and choose which services start](templates-and-profiles.md)
- [Keep personal changes out of Git](local-overrides.md)
- [`extends`](/docs/reference/config/project/extends/#extends)
