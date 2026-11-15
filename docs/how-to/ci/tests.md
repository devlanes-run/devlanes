---
title: Run tests against a lane
description: Start what a test suite needs, run it with the lane's URLs in its environment, and stop what was started.
kind: how-to
---

When tests need the project's services up, such as API or end-to-end tests, one command brings the lane up for the run, hands the tests its URLs and stops what it started.

::example{slug="workflows-ci-with"}

## Read the URLs from the environment

Ports and hostnames differ from lane to lane, so a test that names them works in one worktree only. The command you run gets a variable per service with its URL, named after the service: `DEVLANES_PUBLIC_URL_API` for the API. The smoke test of myapp's API reads it:

```js title="api/smoke.test.mjs"
import assert from "node:assert/strict";
import { test } from "node:test";

test("api answers /healthz", async () => {
  const base = process.env.DEVLANES_PUBLIC_URL_API;
  assert.ok(base, "DEVLANES_PUBLIC_URL_API is set by devlanes exec and devlanes with");
  const res = await fetch(new URL("/healthz", base));
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "ok");
});
```

The URL is the service's HTTPS hostname when it has one. The command also gets variables that point Node, Deno and Python at the devlanes CA, unless your shell already sets them, so `fetch` trusts the proxy. [Environment variables](../../reference/environment-variables.md) lists them all.

## Run the tests

Everything after the two dashes is the command. devlanes starts it directly rather than through a shell, so a pipe or a chain of commands needs a shell of its own, such as `sh -c`, or `cmd /c` on Windows.

```console title="~/dev/myapp"
$ devlanes with -- node --test "api/*.test.mjs"
```

On a stopped lane, the command starts the lane, waits for each service's ready check, runs the tests, then stops the lane again. When the run had to start the daemon too, it stops the daemon at the end as well. Without services or a profile named, the run starts what the lane ran last, or the project's default profile.

## Choose what starts

Name services before the two dashes to start only those. Or pass `--profile` with a profile, a named set of services and their settings, that sets the services up for tests. This one, from the example, runs each service's start script instead of its dev script:

```yaml devlanes
profiles:
  e2e:
    include: ["*"]
    run_scripts:
      dev: start
    env:
      NODE_ENV: test
```

## Read the exit code

The run exits with your command's code, so a CI step fails when the tests do. When a service or task fails, or a service is not ready in time, the command does not run: devlanes prints the end of that log and exits with `1`. [States and exit codes](../../reference/states-and-exit-codes.md) lists the other codes.

The global `--timeout` sets how long the run waits for the services to be ready.

## If the lane already runs

The run starts only the selected services that the lane lacks, runs no tasks, and afterwards stops only what it started, so the lane stays as it was. Services that were running keep the settings they started with. When `--profile` names a profile that would give them other settings, such as another script or other variables, the run exits with `1` before it starts anything. Stop the lane first, run the tests from another worktree, or name the services instead of the profile.

## If you need the logs afterwards

`--logs-dir` writes the log of each service and task the run started into a folder, one file each, named like `unit-api.log`. A rerun replaces its own files and leaves the rest, so a file from an earlier run can sit next to this run's. Start from an empty folder. [Run tests in CI](ci.md) keeps that folder as an artifact.

## If you want to look at a failed run

`--keep` leaves what the run started running, so you can read the logs and open the URLs. When you are done, stop them with `devlanes stop`: with the services' names to stop only those, or with no names for the whole lane.

## Related

- [Run tests in CI](ci.md)
- [A coding agent runs the lane](../../learn/coding-agents.md)
- [Environment variables](../../reference/environment-variables.md)
- [`devlanes with`](../../reference/cli/with.md)
