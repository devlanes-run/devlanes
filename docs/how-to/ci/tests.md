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
  assert.ok(base, "DEVLANES_PUBLIC_URL_API is set by devlanes with");
  const res = await fetch(new URL("/healthz", base));
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "ok");
});
```

The URL is the service's HTTPS hostname when it has one. The command also gets variables that point Node, Deno and Python at the devlanes CA, unless your shell already sets them, so `fetch` trusts the proxy. [Environment variables](../../reference/environment-variables.md) lists them all.

## Run the tests

Everything after the two dashes is the command. devlanes starts it directly rather than through a shell, so a pipe or `&&` needs a shell of its own, such as `sh -c`.

```console title="~/dev/myapp"
$ devlanes with -- node --test "api/*.test.mjs"
myapp@0 (main): 3/3 ready
ready api http://127.0.0.1:20060 https://api.myapp.localhost
ready web http://127.0.0.1:20061 https://myapp.localhost
ready worker http://127.0.0.1:20062
✔ api answers /healthz (40.82ms)
ℹ tests 1
ℹ pass 1
ℹ fail 0
devlanes with · myapp@0 · exit 0
```

Lane 0 was stopped before this run. The command started it, waited for each service's ready check, ran the tests, then stopped the lane again. When the run had to start the daemon too, it stops the daemon at the end as well. Without units or a profile, the run starts what the lane ran last, or the project's default profile.

## Choose what starts

Name services before the two dashes to start only those, or pass `--profile` with a profile that sets the services up for tests. This one, from the example, runs each service's start script instead of its dev script:

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

The run exits with the code of your command, so a CI step or a script can act on it. When devlanes itself stops the run, the code says why:

- `1` when a service or task failed, or a service was not ready before the wait ran out. The command does not run, and devlanes prints the end of the failed one's log.
- `127` when the command does not exist or cannot start.
- `128` plus the signal number when a signal ended the run, after the teardown: `130` for Ctrl-C.

The global `--timeout` sets how long the run waits for readiness. [States and exit codes](../../reference/states-and-exit-codes.md) has the full list.

## If the lane already runs

The run starts only the selected services that the lane lacks, runs no tasks, and afterwards stops only what it started, so the lane stays as it was. Services that were running keep the settings they started with. When `--profile` names a profile that would give them other settings, such as another script or other variables, the run exits with `1` before it starts anything. Stop the lane first, run the tests from another worktree, or name the services instead of the profile.

## If you need the logs afterwards

`--logs-dir` writes the log of each service and task the run started into a folder, one file each, named like `unit-api.log`. Files of the same name are replaced and other files stay, so start from an empty folder when you read it. [Run tests in CI](ci.md) keeps that folder as an artefact.

## If you want to look at a failed run

`--keep` leaves what the run started running, so you can read the logs and open the URLs. Stop it with `devlanes stop` when you are done.

## Related

- [Run tests in CI](ci.md)
- [A coding agent runs the lane](../../learn/coding-agents.md)
- [Environment variables](../../reference/environment-variables.md)
- [`devlanes with`](../../reference/cli/with.md)
