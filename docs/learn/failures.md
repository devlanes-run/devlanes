---
title: When a service fails
description: The API of myapp breaks on the checkout branch while main keeps running, and devlanes shows what broke, what a fix changes and what an edit to the project file waits for.
kind: learn
---

The checkout branch of myapp breaks its API while the main branch keeps running. Later on the same branch, you make a typo in the project file and then edit it while the lane is up. You read each failure in the status and the API's log, and an edit to the project file waits until you reload or restart.

## A failed service stays failed

On the branch, you add a line at the top of the API's server that throws before the server listens. The API's dev script runs Node in watch mode, which waits for the next file change after a crash instead of exiting. You restart the API to run the edited file from a fresh start, and this time its ready check gets no answer and gives up.

```console title="~/dev/myapp-checkout"
$ devlanes restart api --no-ui
myapp@1 · feat/checkout
✗ services  0 of 1 ready
  ✗  api  wasn't ready in time   Failed running 'server.mjs'. Waiting for file changes before restarting...

logs: devlanes logs myapp@1:api --tail 200
$ devlanes status
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  0     main           running  2/2       0.0%  73.7 MiB  https://myapp.localhost
  1     feat/checkout  failed   1/2       0.0%  55.5 MiB  https://myapp1.localhost
```

In the status, lane 1 reads failed with one of its two services ready, and lane 0 on main runs as before. devlanes does not restart a failed service, so the API stays failed, with its reason, until someone restarts or stops it.

## devlanes pulls the stack trace out of the log

`devlanes debug why api` says only that the API did not become ready before the timeout. The cause is in what the API printed, and `devlanes debug errors` takes the stack from the log of each failed unit.

```console title="~/dev/myapp-checkout"
$ devlanes debug errors
failed myapp@1:api
  wasn't ready in time
  | throw new Error("checkout: the api is broken on this branch");
  |       ^
  | 
  | Error: checkout: the api is broken on this branch
  | 
  | Node.js v24.16.0
  | Failed running 'server.mjs'. Waiting for file changes before restarting...
```

## A fix does not clear the failure

You take the broken line out again, and the API still reads failed. The dev script's own watcher may already be serving the fixed file, but devlanes runs a service's ready check when it starts the service and does not look again by itself.

```console title="~/dev/myapp-checkout"
$ devlanes status 1
myapp
  LANE  BRANCH         STATE   SERVICES  CPU   MEM       URL
  1     feat/checkout  failed  1/2       0.6%  74.8 MiB  https://myapp1.localhost

  UNIT            STATE             PORT   CPU   MEM       URL
  api             failed            20070  0.6%  38.2 MiB  https://api.myapp1.localhost
  web             ready             20072  0.0%  36.6 MiB  https://myapp1.localhost
  install         exited            –      –     –
  migrate         exited            –      –     –
  setup.database  exited            –      –     –
  postgres        shared container  50204  1.3%  85.1 MiB
  api: wasn't ready in time
```

A restart settles it: devlanes runs the ready check again, and it passes.

```console title="~/dev/myapp-checkout"
$ devlanes restart api --no-ui
✓ services ready
myapp@1 · feat/checkout
✓ services  1 of 1 ready
  ✓  api  https://api.myapp1.localhost
```

## The project file is checked before it is used

You write `lane.domian` for the lane's domain in the API's hostname, and the config check catches it. It reads the file as a lane would and names the line, the column and the name it expected. Line 35 is the API's `hosts` line in the whole file that [A database for each lane](databases.md) shows.

```console title="~/dev/myapp-checkout, on Windows 11"
$ devlanes config check
error: C:\Users\you\dev\myapp-checkout\devlanes.yaml is invalid at line 35, column 12: services.api.hosts: `lane.domian` is not defined; did you mean `lane.domain`?
hint: edit C:\Users\you\dev\myapp-checkout\devlanes.yaml at line 35, then run this again
```

## An edit shows as drift

With the typo gone, you give the API a log level in its environment.

```yaml devlanes
services:
  api:
    env:
      LOG_LEVEL: debug
```

The lane keeps running as it was. Its status shows drift, the difference between the file and what runs: a note on the lane's row with the command that applies it, and a line under the unit that changed.

```console title="~/dev/myapp-checkout"
$ devlanes status 1
myapp
  LANE  BRANCH         STATE    SERVICES  CPU   MEM       URL
  1     feat/checkout  running  2/2       0.0%  74.0 MiB  https://myapp1.localhost  (config changed: 1 changed; devlanes reload --lane myapp@1 applies it)

  UNIT            STATE             PORT   CPU   MEM       URL
  api             ready             20070  0.0%  37.4 MiB  https://api.myapp1.localhost
  web             ready             20072  0.0%  36.6 MiB  https://myapp1.localhost
  install         exited            –      –     –
  migrate         exited            –      –     –
  setup.database  exited            –      –     –
  postgres        shared container  50204  0.9%  84.9 MiB
  api: env LOG_LEVEL changed since it started
```

## A reload applies only what changed

A reload restarts the units the edit changed, starts the ones it added and stops the ones it removed.

```console title="~/dev/myapp-checkout"
$ devlanes reload --no-ui
✓ services ready
myapp@1 · feat/checkout
✓ services  1 of 1 ready
  ✓  api  https://api.myapp1.localhost
```

The reload counts only what it restarted, the API, and the web app keeps its process. A plain start of this lane would not have applied the edit, since a start leaves running units alone and adds only what the lane lacks. A restart runs the units you name again from the file as it stands, so it would have given the API its log level too. A reload works out the difference for you and restarts only the units it touches.

So far a person has typed every command. Next, a coding agent gets a worktree of its own and runs a lane there.

## On your own project

- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [Troubleshooting](../troubleshooting.md)
- [Config files and reload](../concepts/config-and-reload.md)
- [The daemon](../concepts/daemon.md)
- [States and exit codes](../reference/states-and-exit-codes.md)
