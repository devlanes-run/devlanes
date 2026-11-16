---
title: Run tests against a lane
description: Start what a test suite needs, run it with the lane's URLs in its environment, and stop what was started.
kind: how-to
---

Use this when tests need the project's services up, such as API or end-to-end tests.

::example{slug="workflows-ci-with"}

## Read the URLs from the environment

Ports and hostnames differ per lane, so read each service's URL from `DEVLANES_PUBLIC_URL_<SERVICE>`, such as `DEVLANES_PUBLIC_URL_API` for the API:

```js title="api/smoke.test.mjs"
import assert from "node:assert/strict";
import { test } from "node:test";

test("api answers /healthz", async () => {
  const res = await fetch(new URL("/healthz", process.env.DEVLANES_PUBLIC_URL_API));
  assert.equal(res.status, 200);
});
```

The tests also get variables that point Node, Deno and Python at the devlanes CA, so they trust the HTTPS URLs. [Environment variables](../../reference/environment-variables.md) lists them.

## Run the tests

Put the test command after the two dashes:

```console
$ devlanes with -- node --test "api/*.test.mjs"
```

devlanes starts what the lane needs, waits until the services are ready, runs the tests, stops what it started and exits with the tests' exit code. On a lane that already runs, it starts only what is missing and leaves the rest running. When a service fails to start, the tests do not run and the command exits with `1`.

## If the tests need other settings

Name a profile, a named set of services and their settings, such as one that runs each service's start script instead of its dev script:

```yaml devlanes
profiles:
  e2e:
    include: ["*"]
    run_scripts:
      dev: start
```

```console
$ devlanes with --profile e2e -- npm test
```

## If you need the logs afterwards

`--logs-dir` writes one log file per service and task the run started, such as `unit-api.log`, into the folder you name. [Run tests in CI](ci.md) keeps that folder.

## Related

- [Run tests in CI](ci.md)
- [Run a command with a lane's environment](../services/one-off-commands.md)
- [A coding agent runs the lane](../../learn/coding-agents.md)
- [States and exit codes](../../reference/states-and-exit-codes.md)
- [`devlanes with`](../../reference/cli/with.md)
