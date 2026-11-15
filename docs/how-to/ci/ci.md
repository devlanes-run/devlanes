---
title: Run tests in CI
description: Install devlanes in a CI job, run the tests against a lane and keep the logs when something fails.
kind: how-to
---

A CI job can run your tests against the same project file and the same services as your machine, with the lane started on the runner and its logs kept when the job fails. The tests need to read their URLs from the environment, as [Run tests against a lane](tests.md) shows.

::example{slug="workflows-ci-with"}

## Add the action to a workflow

On GitHub Actions, the devlanes action installs devlanes, prepares the runner and runs your tests through `devlanes with`. Install the project's dependencies in a step before it, since a CI checkout starts without them:

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

The step fails when the lane does not start or the tests fail. Pin `version` to the release your team runs. Without it, the action takes the version from a lockfile that names devlanes, or else installs the release it was tested with, which can be several releases behind.

The checkout becomes lane 0 of the project whatever its branch, since CI checks pull requests out detached, so its hostnames are those of the main worktree on your machine. devlanes notices CI by its variables: it prints plain output, never asks a question and makes no release checks. The action also turns off throttling and freezing, so a long step does not suspend the lane.

## Decide whether the runner trusts the CA

On Linux runners the action runs the setup command, which adds the devlanes CA to the system store and to the store Chromium reads, Playwright's included. It also lets the proxy listen on port 443, so URLs carry no port. The tests get variables that point Node, Deno and Python at the CA either way, so a suite that calls the services only from those runtimes can skip the trust step:

```yaml
      - uses: devlanes-run/action@v0
        with:
          command: with
          run: npm test
          trust: false
```

macOS and Windows runners cannot trust the CA, because their trust store asks in a dialog nobody can answer there. Browser tests on them need the CA file from the action's `ca-cert` output or have to ignore certificate errors.

## Keep the logs

When a step of the job fails, the action uploads an artifact with each unit's log, the daemon's log and the output of `devlanes doctor`. It asks the GitHub API which step failed, which is what `actions: read` in the workflow allows. To keep the logs after every run:

```yaml
      - uses: devlanes-run/action@v0
        with:
          command: with
          run: npm test
          upload-logs: always
```

In the job log, devlanes puts each phase of the start in a group of its own and adds an annotation with the last log lines of each failed unit. The step summary gets a table of each unit's run time, CPU and memory.

## Set the timeouts

The start waits for every service to be ready and gives up after a default of a few minutes, which a first build can outgrow. The action's `timeout` input raises it. devlanes puts no limit on the tests themselves, so `timeout-minutes` on the job is what ends a run that hangs.

```yaml
      - uses: devlanes-run/action@v0
        with:
          command: with
          run: npm test
          timeout: 5m
```

## If later steps need the lane

Without `command`, the action starts the lane and leaves it running for the steps after it. Later steps get the lane's variables, so the tests read the same URLs as in the workflow above. At the end of the job the action stops the lane and the daemon and uploads the logs as `upload-logs` says.

```yaml
      - uses: devlanes-run/action@v0
        with:
          profile: e2e
      - run: npm test
```

## If you use another CI system

Install devlanes with the script, set up HTTPS without a question, install the dependencies and run the tests. The script installs into `~/.local/bin`, which has to be on the job's `PATH`.

```console
$ curl -fsSL https://devlanes.run/install.sh | sh
$ devlanes config set tls.trust "[system, chrome]"
$ devlanes setup --yes
$ npm ci
$ devlanes with --profile e2e --logs-dir devlanes-logs -- npm test
```

On Linux the setup updates the system store through sudo, so the job needs root or sudo without a password. The second line adds Chromium's store to the system store, which needs `certutil` from the NSS tools; leave it out when no browser runs. Unless the daemon may bind port 443, the proxy listens on 8443 and URLs carry that port, which [Choose the proxy's ports](../hostnames/proxy-ports.md) changes.

Keep the logs folder with your CI's own artifact step, set to run when the job fails. The groups, annotations and step summary are for GitHub Actions only.

## If the runner is self-hosted

Lanes, ports and containers outlive a job on a runner that is not thrown away. The action's README covers a devlanes home per job and cleaning the lane at the end: [devlanes-run/action](https://github.com/devlanes-run/action#self-hosted-runners).

## Related

- [Run tests against a lane](tests.md)
- [Trust the devlanes CA](../hostnames/trust-the-ca.md)
- [Choose the proxy's ports](../hostnames/proxy-ports.md)
- [devlanes-run/action](https://github.com/devlanes-run/action)
