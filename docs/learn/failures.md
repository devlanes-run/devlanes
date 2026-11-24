---
title: When a service fails
description: The API of myapp breaks on the checkout branch while main keeps running, and you find the reason, fix it, check the project file and apply an edit.
kind: learn
---

The checkout branch of myapp breaks its API while the main branch keeps running. On the same branch you then make a typo in the project file and change it while the lane runs.

## A failed service stays failed

On the branch, you add a line at the top of the API's server that throws before the server listens.

```js
// api/server.mjs
throw new Error("checkout: the api is broken on this branch");
```

```console title="~/dev/myapp-checkout"
$ devlanes restart api --no-ui
$ devlanes status
```

The restart waits for the API's ready check, which never passes, so the API reads failed and lane 1 with it, while lane 0 on main keeps running. devlanes does not restart a failed service: it stays failed until you restart or stop it.

## The log holds the reason

```console title="~/dev/myapp-checkout"
$ devlanes debug errors
```

This prints each failed unit with the end of its log, here the API's stack trace pointing at the line you added. `devlanes logs api` shows the whole log.

You take the line out again. The API still reads failed, because devlanes runs the ready check when it starts a service and not later.

```console title="~/dev/myapp-checkout"
$ devlanes restart api --no-ui
```

This time the ready check passes and the API reads ready.

## The project file is checked

You misspell the lane's domain in the API's hostname.

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domian }}"
```

```console title="~/dev/myapp-checkout"
$ devlanes config check
```

The check names the file, the line and the column, and suggests `lane.domain`. You fix the line.

## A reload applies an edit

You give the API a log level.

```yaml devlanes
services:
  api:
    env:
      LOG_LEVEL: debug
```

```console title="~/dev/myapp-checkout"
$ devlanes status
$ devlanes reload --no-ui
```

The lane keeps running as it was, and its status says the config changed and names the API. The reload restarts the API alone, and the web app keeps running.

A start leaves running services alone and adds only those the lane lacks. A restart runs the services you name again from the file as it stands. A reload applies the difference between the file and what runs.

A person has typed every command so far. Next, a coding agent gets a worktree of its own and runs a lane there.

## On your own project

- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [Troubleshooting](../troubleshooting.md)
- [Config files and reload](../concepts/config-and-reload.md)
- [The daemon](../concepts/daemon.md)
