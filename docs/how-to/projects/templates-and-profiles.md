---
title: Share settings and choose which services start
description: Write settings once for services that share them, group services with tags and start a chosen set of them with profiles.
kind: how-to
---

Use this guide when a project has many services that repeat the same settings, and you rarely need all of them running at once.

::example{slug="workflows-profiles-tags"}

## Share settings, tag services and group them

A template is a named set of service settings that services take over with `extends`. A tag is a label on a service, and `+` with its name selects every service that has it. A profile is a named selection of services to start:

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
    tags: [api, essential]
  mailer:
    extends: server
    run_script: { file: apps/mailer/package.json }
    tags: [worker, essential]
profiles:
  default: [+essential]
  backend: [+api, +worker]
```

Each service keeps the template's script name and adds its own file; its own settings win, as [`services.<name>.extends`](/docs/reference/config/project/services/#services-name-extends) describes. The profile named `default` is what a plain start runs on a lane's first start and after you stop the whole lane.

## Start with a profile

```console title="~/dev/myapp"
$ devlanes start --profile backend
```

The start runs the services the profile selects, and the lane remembers the profile, so a plain start after a reboot brings back the same selection. Stopping the whole lane makes it forget the profile:

```console title="~/dev/myapp"
$ devlanes stop
```

## If a profile needs its own variables

A profile written as a mapping can set `env`. While the profile is active, those variables reach every unit of the lane, which means each service and task, including services the profile does not select. A service's own `env` cannot override them:

```yaml devlanes
profiles:
  test:
    include: [+essential]
    env:
      DATA_FILE: "${{ lane.data_path }}/test/${{ self.name }}.json"
```

## If a profile should skip the containers

Set `infra: false` on the profile. [Point a frontend at another lane's backend](../lanes/attach-to-another-lane.md) builds on that.

## Related

- [Start a service only on demand](../services/on-demand.md)
- [Run dev servers and a production build side by side](dev-and-prod.md)
- [Lane and unit references](../../reference/lane-and-unit-references.md)
