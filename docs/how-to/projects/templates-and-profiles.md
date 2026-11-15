---
title: Share settings and choose which services start
description: Write settings once for services that share them, group services with tags and start a chosen set of them with profiles.
kind: how-to
---

Use this guide when a project has many services that repeat the same settings, and you rarely need all of them running at once.

::example{slug="workflows-profiles-tags"}

## Put shared settings in a template

A template is a named set of service settings that services take over with `extends`. The service's own settings win, and a template can extend another:

```yaml devlanes
templates:
  server:
    run_script:
      name: dev
    ready:
      http:
        path: /healthz
services:
  accounts-api:
    extends: server
    run_script: { file: apps/accounts-api/package.json }
    hosts: "accounts.${{ lane.domain }}"
```

Mappings such as the script and the ready check merge by key, so this service keeps the template's script name and adds its own file. Lists and single values replace the template's. [`services.<name>.extends`](/docs/reference/config/project/services/) has the full rules.

## Tag the services

A tag is a label on a service, and `+` with its name selects every service that has it. Services list their tags; the top-level list is optional and gives a tag a colour or a description:

```yaml devlanes
tags:
  api: { color: cyan }
  web: { color: green }
  worker: { color: magenta }
services:
  accounts-api:
    tags: [api, essential, auth]
  shop-web:
    tags: [web, essential, shop]
  mailer:
    tags: [worker, essential, email]
```

A service takes the colour of the first of its tags that has one, so list the kind of service first and the tags you select by after it. A tag works on the command line as well, as in `devlanes start +api`.

## Group the services in profiles

A profile is a named selection of services to start: names, globs and tags, written as a list, or as a mapping that can also exclude services:

```yaml devlanes
profiles:
  default: [+essential]
  backend: [+api, +worker]
  all:
    include: ["*"]
    exclude: [+worker]
```

A selector that matches no service makes the file invalid, and `devlanes config check` names the profile. A glob leaves out services whose autostart is off, while a name or a tag includes them, which [Start a service only on demand](../services/on-demand.md) uses.

The profile named `default` is what a plain start runs when the lane has nothing to bring back, on its first start and after the whole lane was stopped. With [`start.restore`](/docs/reference/config/user/start/) set to false, every plain start runs it. A project without one starts every service whose autostart is on.

## Start with a profile

Name the profile with `--profile` on the start, and repeat the flag to combine several; where two set the same variable, the later one wins. The lane remembers its profiles, so a plain start after a reboot brings back the same selection. Stopping the whole lane makes it forget them, and its next plain start runs the default profile again.

On a lane that already runs, the start adds the profile's services that the lane lacks. Services that already run keep the settings they started with, and the start warns about those the profile would change. `devlanes status` lists them, and `devlanes reload` restarts them with the profile.

## If a profile needs its own variables

A profile written as a mapping can set variables for its services. They also reach the lane's tasks and its `infra.setup` commands, and the variables a service or task sets itself cannot override them:

```yaml devlanes
profiles:
  test:
    include: [+essential]
    env:
      DATA_FILE: "${{ lane.data_path }}/test/${{ self.name }}.json"
```

Put a variable that some task needs to change, such as a production mode next to an install task, in `global_env` instead, where the task's own `env` can override it.

## If a profile should skip the containers

Set `infra: false` on the profile, and its start leaves out the containers and the commands that set them up for the lane. [Point a frontend at another lane's backend](../lanes/attach-to-another-lane.md) builds on that.

## If a profile runs other scripts

A profile can swap the script each service runs, such as `start` for `dev`. [Run dev servers and a production build side by side](dev-and-prod.md) shows it.

## Related

- [Keep personal changes out of Git](local-overrides.md)
- [Lane and unit references](../../reference/lane-and-unit-references.md)
- [Lanes and projects](../../concepts/lanes-and-projects.md)
