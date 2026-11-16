---
title: Run tests in CI
description: Install devlanes in a CI job, run the tests against a lane and keep the logs when something fails.
kind: how-to
---

Use this to run the tests from [Run tests against a lane](tests.md) in a CI job, with the same project file as on your machine.

::example{slug="workflows-ci-with"}

## Add the action to a workflow

On GitHub Actions, the devlanes action installs devlanes, trusts its CA on Linux runners and runs your tests through `devlanes with`. Install the project's dependencies in a step before it:

```yaml title=".github/workflows/e2e.yml"
name: e2e

on:
  pull_request:

permissions:
  contents: read
  actions: read

jobs:
  e2e:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v7
      - uses: actions/setup-node@v7
        with:
          node-version: 24
      - run: npm ci
      - uses: devlanes-run/action@v0
        with:
          command: with
          profile: e2e
          run: npm test
          version: 0.13.0
```

The step fails when the lane does not start or the tests fail. Pin `version` to the release your team runs. The checkout becomes lane 0 whatever its branch, so its hostnames are those of the main worktree on your machine.

## Keep the logs

When a step fails, the action uploads an artifact with each unit's log, the daemon's log and the output of `devlanes doctor`. The `actions: read` permission lets it see which step failed. To upload after every run, add `upload-logs: always` to the action's inputs.

## If a first start is slow

The start waits a few minutes for the services to be ready. Raise that with the action's `timeout` input, such as `timeout: 5m`. Keep `timeout-minutes` on the job, since nothing else ends tests that hang.

## If you use another CI system

Install devlanes with the script, set up HTTPS without a question, install the dependencies and run the tests:

```console
$ curl -fsSL https://devlanes.run/install.sh | sh
$ devlanes setup --yes
$ npm ci
$ devlanes with --profile e2e --logs-dir devlanes-logs -- npm test
```

On Linux the setup needs root or sudo without a password. Keep the `devlanes-logs` folder with your CI's artifact step, set to run when the job fails.

## Related

- [Run tests against a lane](tests.md)
- [Trust the devlanes CA](../hostnames/trust-the-ca.md)
- [Choose the proxy's ports](../hostnames/proxy-ports.md)
- [devlanes-run/action](https://github.com/devlanes-run/action)
