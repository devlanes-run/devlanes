---
title: A coding agent runs the lane
description: A coding agent works on a branch of myapp in its own worktree, runs a lane there next to the lanes a person started, and stops only what it started.
kind: learn
---

A person started lanes 0 and 1 of myapp. A coding agent works on a branch of its own, agent/perf, in a third worktree. It runs its lane through the same daemon and the same commands, and devlanes keeps track of which units are the agent's.

## The agent's start becomes lane 2

The agent adds a worktree for its branch next to the other two, as in [Every worktree gets a lane](worktrees.md).

```console title="~/dev/myapp"
$ git worktree add ../myapp-perf -b agent/perf
Preparing worktree (new branch 'agent/perf')
HEAD is now at fd2054b Add a database for each lane
```

A dev server an agent starts in its shell's background tends to die with that shell, so the agent hands its services to the daemon with a start. devlanes sees from the variables the agent's tool sets that an agent is running it, and answers in a terser format, with one line per service.

```console title="~/dev/myapp-perf, as the agent sees it"
$ devlanes start
myapp@2 (agent/perf): 2/2 ready
ready api http://127.0.0.1:20080 https://api.myapp2.localhost
ready web http://127.0.0.1:20082 https://myapp2.localhost
```

The agent learns these commands from the devlanes skill, which [Set up a coding agent](../how-to/agents/set-up.md) installs.

## A command of its own becomes a unit

The agent wants the tests to run again on every change, which no service in the project file does. `devlanes exec` runs the watcher as a unit of the lane, so its process and its log belong to the daemon and not to the agent's shell.

```console title="~/dev/myapp-perf, as the agent sees it"
$ devlanes exec --name watch-tests -- node --test --watch "api/*.test.mjs"
running myapp@2:watch-tests actor=opencode pid=53356
  logs: devlanes logs myapp@2:watch-tests --tail 100
  stop: devlanes stop myapp@2:watch-tests
```

The unit gets the lane's environment, like a service. A unit runs under your account and outside any sandbox the agent's shell is in, so a command an agent hands to devlanes can read and change what you can. [Security](../concepts/security.md) covers what that means.

## The person's units carry no label

The label at the end of the exec line is the daemon's record of who started the unit. Lane 0, where a person typed the start, has none.

```console title="~/dev/myapp-perf, as the agent sees it"
$ devlanes status 2
running myapp@2 agent/perf 2/2
ready myapp@2:api http://127.0.0.1:20080 https://api.myapp2.localhost default=20080 actor=opencode
ready myapp@2:web http://127.0.0.1:20082 https://myapp2.localhost default=20082 actor=opencode
exited myapp@2:install exit=0 actor=opencode
exited myapp@2:migrate exit=0 actor=opencode
exited myapp@2:setup.database exit=0 actor=opencode
running myapp@2:watch-tests actor=opencode
$ devlanes status 0
running myapp@0 main 2/2
ready myapp@0:api http://127.0.0.1:20060 https://api.myapp.localhost default=20060
ready myapp@0:web http://127.0.0.1:20062 https://myapp.localhost default=20062
```

Lane 2 has run its install task, its setup and its migration, so it has a database of its own. The field marked `default` is the service's main port. The label is the agent's name, here OpenCode. The daemon also records which session of the agent started each unit, when the agent's tool reports one, and the agent's stop below matches that session.

## Tests run against the lane that is up

The smoke test reads the API's URL from `DEVLANES_PUBLIC_URL_API`, which devlanes sets in a lane's environment for each service, as the environment in [Services that find each other](connecting-services.md) shows. So the test names no port and passes in any lane. `devlanes with` runs a command with the lane's environment, that variable included, and exits with the command's exit code.

```js
// api/smoke.test.mjs
test("api answers /healthz", async () => {
  const base = process.env.DEVLANES_PUBLIC_URL_API;
  const res = await fetch(new URL("/healthz", base));
  assert.equal(res.status, 200);
});
```

```console title="~/dev/myapp-perf, as the agent sees it"
$ devlanes with -- node --test "api/*.test.mjs"
myapp@2 (agent/perf): 2/2 ready, already running (nothing started)
✔ api answers /healthz (40.9015ms)
ℹ tests 1
ℹ pass 1
ℹ fail 0
```

The lane runs, so `with` starts and stops nothing. On a stopped lane it starts the lane first and stops what it started afterwards, which [Run tests against a lane](../how-to/ci/tests.md) shows.

## The agent stops only what it started

Before it hands the branch back, the agent stops its own units. With no unit named, `devlanes stop --mine` covers the units of this project that the agent's session started.

```console title="~/dev/myapp-perf, as the agent sees it"
$ devlanes stop --mine
stopped myapp@2:api
stopped myapp@2:watch-tests
stopped myapp@2:web
$ devlanes status
running myapp@0 main 2/2
running myapp@1 feat/checkout 2/2
idle myapp@2 agent/perf
```

Only lane 2's units are on the list. The person's lanes 0 and 1 keep running, and lane 2 is idle: it runs nothing and keeps its ports and its database. Leaving the person's lanes alone is the agent's choice, which the skill teaches; devlanes lets any process of your account stop them.

Every lane so far runs on this machine. Next, myapp's lanes move onto a box.

## On your own project

- [Set up a coding agent](../how-to/agents/set-up.md)
- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Run tests against a lane](../how-to/ci/tests.md)
- [The daemon](../concepts/daemon.md)
- [Security](../concepts/security.md)
