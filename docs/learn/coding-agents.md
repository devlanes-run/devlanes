---
title: A coding agent runs the lane
description: A coding agent starts, tests and stops a lane of myapp on its own, and devlanes answers it in a shorter format.
kind: learn
---

myapp has an API, a web app and a worker, and both of its lanes are up. A coding agent takes over the main worktree and runs lane 0 by itself: it starts the services, adds a process of its own, runs a test and stops what it started, while lane 1 keeps serving the checkout branch.

## The agent starts the lane through the daemon

An agent runs each command in a shell that may be gone a moment later, so a dev server it starts in the background of that shell tends to die with it or be forgotten. The devlanes skill, installed with `npx skills add devlanes-run/devlanes --skill devlanes`, teaches the agent to hand its services to the daemon instead, with the same commands a person uses. [Set up a coding agent](../how-to/agents/set-up.md) also covers the MCP server, for agents whose shell cannot reach the daemon.

```console title="~/dev/myapp"
$ devlanes start
myapp@0 (main): 3/3 ready, already running (nothing started)
ready api http://127.0.0.1:20060 https://api.myapp.localhost
ready web http://127.0.0.1:20061 https://myapp.localhost
ready worker http://127.0.0.1:20062
```

The lane was already up, so the start left it alone and said so. This is the format devlanes prints when it recognizes an agent by the variables its host sets: a line per service with its local port and its hostname, and no prompts. When something fails, the failure comes first with the end of its log and the command that shows more, and the output stays under a size limit that [States and exit codes](../reference/states-and-exit-codes.md) gives.

## A command of its own becomes a unit

Some processes the agent needs are not in the project file, such as a watcher or a server for one experiment. Run through `devlanes exec`, such a process belongs to the daemon like a service, gets the lane's environment and keeps its output.

```console title="~/dev/myapp"
$ devlanes exec --name ticker -- node -e "setInterval(() => console.log('tick'), 2000)"
running myapp@0:ticker actor=opencode pid=8592
  logs: devlanes logs myapp@0:ticker --tail 100
  stop: devlanes stop myapp@0:ticker
```

The name is what later commands use to find it, and the answer spells out the two the agent will want next. A unit made this way gets no port or hostname of its own.

## Each unit records who started it

Every request names who sent it, a person or an agent session, and the daemon writes that on each unit it starts. In the lane's status, the label sits at the end of each line.

```console title="~/dev/myapp"
$ devlanes status 0
running myapp@0 main 3/3
ready myapp@0:api http://127.0.0.1:20060 https://api.myapp.localhost default=20060 actor=opencode
ready myapp@0:web http://127.0.0.1:20061 https://myapp.localhost default=20061 actor=opencode
ready myapp@0:worker http://127.0.0.1:20062 default=20062 actor=opencode
running myapp@0:ticker actor=opencode
```

This agent's session started every unit of lane 0, ticker included. The default entry is the service's port under its name. The client sets the label itself and the daemon does not check it, so it tells sessions apart without guarding anything.

A lane is not a sandbox. Every process in it runs as you, with your files and your network, whether a person or an agent started it. [Security](../concepts/security.md) covers what that means.

## The agent stops only what it started

Before handing the work back, the agent stops ticker by name, then everything else it started in this lane.

```console title="~/dev/myapp"
$ devlanes stop --mine "*"
stopped myapp@0:api
stopped myapp@0:web
stopped myapp@0:worker
```

The star covers the units of the current lane, and the flag keeps the ones this session started. Units a person started would have kept running, and so did lane 1, although the same session had started it. Without a unit, the flag covers every lane of the project.

## A test gets the lane for as long as it runs

The API has a smoke test that reads the API's URL from `DEVLANES_PUBLIC_URL_API`, so it names no port. `devlanes with` sets that variable for each service while it runs a command against the lane.

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

Lane 0 was stopped when the run began. It started the lane, waited until each service was ready, ran the test, then stopped the lane again and exited with the test's exit code.

## On your own project

- [Set up a coding agent](../how-to/agents/set-up.md)
- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Run tests against a lane](../how-to/ci/tests.md)
- [The daemon](../concepts/daemon.md)
- [Security](../concepts/security.md)
