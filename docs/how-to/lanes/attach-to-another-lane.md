---
title: Point a frontend at another lane's backend
description: Run only the frontend in a feature worktree and have it call the API that lane 0 already runs.
kind: how-to
---

Use this when a branch changes only the frontend and you want it to call the API of the main lane instead of starting a second backend.

::example{slug="workflows-profiles-tags"}

## Add a profile that reads lane 0

A profile is a named set of services and settings a start can pick. This one starts only the web service and gives it lane 0's API through `lanes.main`.

```yaml devlanes
profiles:
  attach:
    include: [web]
    infra: false
    env:
      VITE_API_URL: "${{ lanes.main.services.api.public_url }}"
```

The profile's variable replaces the web service's own. The `infra: false` line skips the lane's containers and setup; its start tasks, such as a migration, still run.

## Start the API in lane 0

```console title="~/dev/myapp-checkout"
$ devlanes start 0:api --no-ui
```

## Start the frontend alone

Stop the feature lane first if it runs, since a start does not change services that already run. Then start the profile.

```console title="~/dev/myapp-checkout"
$ devlanes stop
$ devlanes start --profile attach --no-ui
```

The web service of lane 1 starts and calls the API of lane 0.

## If the API checks the origin

Each lane domain is a site of its own, so an API that allows only its own lane's origins refuses the page of lane 1. [Set CORS and cookies for each lane](../hostnames/cors-and-cookies.md) covers allowing it.

## If the backend is another project

Read it from the `projects` namespace, with a fallback for when that project runs nothing.

```yaml devlanes
profiles:
  attach:
    include: [web]
    env:
      VITE_API_URL: "${{ projects.billing.services.api.public_url | default('https://billing.example.invalid') }}"
```

## Related

- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Expressions](../../reference/expressions.md)
- [Lane and unit references](../../reference/lane-and-unit-references.md)
