---
title: A coding agent runs the lane
description: A coding agent works on a branch of myapp in its own worktree, runs a lane there next to the lanes a person started, and stops only what it started.
kind: learn
---

A person started lanes 0 and 1 of myapp. A coding agent takes a branch of its own, agent/perf, and runs its lane with the same daemon and the same commands.

## The agent gets a worktree and a lane

```console title="~/dev/myapp"
$ git worktree add ../myapp-perf -b agent/perf
```

```console title="~/dev/myapp-perf"
$ devlanes start
```

The worktree becomes lane 2, with its own database and its own hostnames, `api.myapp2.localhost` and `myapp2.localhost`. A dev server an agent starts in its shell's background often dies with that shell, so the agent hands its services to the daemon instead. devlanes notices that an agent runs it and answers in a shorter format meant for agents.

The agent learns these commands from the devlanes skill, which [Set up a coding agent](../how-to/agents/set-up.md) installs.

## A command of its own becomes a unit

The agent wants the tests to run again on every change, and no service in the project file does that.

```console title="~/dev/myapp-perf"
$ devlanes exec --name watch-tests -- node --test --watch "api/*.test.mjs"
$ devlanes status 2
```

The watcher becomes a unit of lane 2, so the daemon keeps it and its log after the agent's shell is gone. The status labels each unit of lane 2 with the agent's name. The units of lanes 0 and 1, which a person started, carry no label.

A unit runs under your account and outside any sandbox the agent's shell is in, so a command an agent hands to devlanes can read and change what you can. [Security](../concepts/security.md) says more.

## Tests run against the lane

The smoke test reads the API's URL from a variable devlanes sets for each service, so it works in any lane.

```js
// api/smoke.test.mjs
test("api answers /healthz", async () => {
  const base = process.env.DEVLANES_PUBLIC_URL_API;
  const res = await fetch(new URL("/healthz", base));
  assert.equal(res.status, 200);
});
```

```console title="~/dev/myapp-perf"
$ devlanes with -- node --test "api/*.test.mjs"
```

The test runs with lane 2's environment and passes. The lane was already running, so `devlanes with` starts and stops nothing; on a stopped lane it starts the lane first and stops it again afterwards.

## The agent stops only what it started

```console title="~/dev/myapp-perf"
$ devlanes stop --mine
```

This stops the units the agent's session started: lane 2's services and the watcher. Lanes 0 and 1 keep running. Leaving them alone is the agent's choice, which the skill teaches; devlanes would let any process of your account stop them.

Every lane so far runs on this machine. Next, the main branch of myapp moves onto a box.

## On your own project

- [Set up a coding agent](../how-to/agents/set-up.md)
- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Run tests against a lane](../how-to/ci/tests.md)
- [Security](../concepts/security.md)
