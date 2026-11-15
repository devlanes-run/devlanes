---
title: Point a frontend at another lane's backend
description: Run only the frontend in a feature worktree and have it call the API that lane 0 already runs.
kind: how-to
---

Use this when a branch changes only the frontend and you would rather reuse the backend the main lane already runs than start a second one with its own containers and data.

::example{slug="workflows-profiles-tags"}

## Add a profile that reads lane 0

The web service of myapp normally reads the API of its own lane. A profile, a named set of services and settings that a start can pick, can start the web service alone and hand it lane 0's API instead, through the `lanes.main` namespace of expressions.

```yaml devlanes
services:
  web:
    env:
      VITE_API_URL: "${{ services.api.public_url }}"

profiles:
  attach:
    include: [web]
    infra: false
    env:
      VITE_API_URL: "${{ lanes.main.services.api.public_url }}"
```

A profile's variables sit above a service's own, so the web service gets lane 0's URL while the profile is active. The `infra: false` line skips the lane's containers and setup, which a frontend talking to another lane does not need. A call made from a server rather than the browser reads the other URL of the API, as in [Services that find each other](../../learn/connecting-services.md).

No other lane has a namespace like this one: an expression cannot read lane 2, or a lane by its branch. In lane 0 itself, the profile reads lane 0's own API.

## Start the API in lane 0

The value resolves even while lane 0 is stopped, and the calls fail until its API is up. A unit reference can carry its lane, so this works from any worktree of the project.

```console title="~/dev/myapp-checkout"
$ devlanes start 0:api --no-ui
```

Values that read the containers, such as myapp's database URL, still need the shared containers running, and lane 0's start brings them up.

## Stop the feature lane

A start into a lane that already runs does not give its running services the profile's variables, so the web service would keep calling lane 1's API. Stop the lane first if it runs.

```console title="~/dev/myapp-checkout"
$ devlanes stop
```

## Start the frontend alone

Start the profile in place of the lane's usual services. Only the web service of lane 1 starts, its page calls the API of lane 0, and the API of lane 1 stays stopped.

```console title="~/dev/myapp-checkout"
$ devlanes start --profile attach --no-ui
```

A later plain start in this worktree brings back the same profile. After a whole-lane stop, a plain start returns to the lane's default services.

## If the API checks the origin

The page of lane 1 and the API of lane 0 are on different sites, since each lane domain is a site of its own. An API that allows only its own lane's origins, as one reading `DEVLANES_ORIGINS` does, refuses the page, and a cookie it sets is a third-party cookie there. [Set CORS and cookies for each lane](../hostnames/cors-and-cookies.md) covers allowing the other lane.

## If the backend is another project

When the API belongs to another devlanes project, read it from the `projects` namespace, with a fallback for when that project runs nothing. [Expressions](../../reference/expressions.md) says which of its lanes the value comes from.

```yaml devlanes
profiles:
  attach:
    include: [web]
    env:
      VITE_API_URL: "${{ projects.billing.services.api.public_url | default('https://billing.example.invalid') }}"
```

## Related

- [Share settings and choose which services start](../projects/templates-and-profiles.md)
- [Environment and expressions](../../concepts/environment.md)
- [Lane and unit references](../../reference/lane-and-unit-references.md)
