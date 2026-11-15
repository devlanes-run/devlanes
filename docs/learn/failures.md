---
title: When a service fails
description: The API of myapp fails and its project file changes while the lanes run, and devlanes shows what broke and what an edit would change.
kind: learn
---

myapp now runs a worker next to the API and the web app: a Node script that listens on no port and prints a line once it is ready. Then the API breaks, first in its code and then in its ready check, and the project file changes while both lanes are up.

## A failed service stays failed

A line added to the API's server throws an error at startup. Its dev script runs Node in watch mode, which keeps the process alive after the crash, so a restart of the API waited the full minute a ready check allows by default before it gave up.

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE   SERVICES  CPU   MEM       URL
  0     main    failed  2/3       0.0%  74.6 MiB  https://myapp.localhost

  UNIT    STATE   PORT   CPU   MEM       URL
  api     failed  20060  0.0%  18.8 MiB  https://api.myapp.localhost
  web     ready   20061  0.0%  43.1 MiB  https://myapp.localhost
  worker  ready   20062  0.0%  12.7 MiB  http://127.0.0.1:20062
  api: wasn't ready in time
```

One failed service marks the whole lane failed, and the other two keep running. The worker has a port and a local URL like the others, although nothing listens there.

devlanes does not restart a failed service. It stays failed, with its reason, until a start, a restart or a stop of it. Removing the bad line did not change that: the API's watcher ran the fixed file, yet the status still said failed until the next restart, which came back ready.

## The reason is kept with the service

The restart printed only the last line of the API's log, which was the watcher's. The error itself is further up, and the debug errors command pulls it out for each failed unit.

```console title="~/dev/myapp"
$ devlanes debug errors
failed myapp@0:api
  wasn't ready in time
  | throw new Error("config.json is missing");
  |       ^
  | 
  | Error: config.json is missing
  | 
  | Node.js v24.16.0
  | Failed running 'server.mjs'. Waiting for file changes before restarting...
```

The same lines are in the API's log, which keeps the output of every run of the service. `devlanes debug why api` gives the cause in one line, with a fix when it knows one.

## A failed ready check leaves the process running

The second break is in the project file: the API's ready check asks for a path the API does not serve, with ten seconds to pass. The restart failed after those ten seconds, naming the 404 the check got, with a hint on how to fix the check.

The API itself was fine and went on running, and it kept its port. The debug errors command named that port as held by a process ID, which was the failed API's own. The proxy does not send requests to a failed service and answers with a page of its own. The curl flag lets Windows curl accept a certificate whose revocation it cannot check.

```console title="on Windows 11"
$ curl.exe -s -i --ssl-revoke-best-effort https://api.myapp.localhost/
HTTP/1.1 503 Service Unavailable
x-devlanes-error: service-failed

# DL203 Service failed

`service-failed`, 503 Service Unavailable, error. Sent by the devlanes 0.13.0 proxy, not by the service.

The service exited or its ready check failed.

Recent output:

api listening on http://127.0.0.1:20060

## Diagnose

- Show the cause and a fix: `devlanes debug why myapp@0:api`
- Read its output since the last start: `devlanes logs myapp@0:api --since start`
- Restart it: `devlanes restart myapp@0:api`
```

A browser gets the same page as HTML. A hostname that no lane routes gets an unknown host page with the hostnames that are routed, over `http://` only. Over `https://` the proxy has no certificate for that name and refuses the connection, so the client shows a TLS error and no page.

## The project file is checked before it is used

A typo in the file, the API's hostname line written with the wrong key, is caught by the config check, which names the line and the key it expected.

```console title="~/dev/myapp"
$ devlanes config check
error: C:\Users\you\dev\myapp\devlanes.yaml is invalid at line 14, column 11: services.api.host: unknown key `host`; did you mean `hosts`?
hint: edit C:\Users\you\dev\myapp\devlanes.yaml at line 14, then run this again
```

The running lane did not stop. Its status kept the three services up and showed the same error on the lane's row. An expression that names a service the file does not have is caught the same way.

## An edit waits for a reload

The worker gains a variable with the API's local URL, the kind a call from one service to another uses. Saving the file restarts nothing; the status lists what differs from what is running.

```console title="~/dev/myapp"
$ devlanes status 0
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  3/3       0.0%  92.7 MiB  https://myapp.localhost  (config changed: 1 changed; devlanes reload --lane myapp@0 applies it)

  UNIT    STATE  PORT   CPU   MEM       URL
  api     ready  20060  0.0%  36.9 MiB  https://api.myapp.localhost
  web     ready  20061  0.0%  43.1 MiB  https://myapp.localhost
  worker  ready  20062  0.0%  12.7 MiB  http://127.0.0.1:20062
  worker: env API_URL changed since it started
```

The reload restarted the worker alone, and the API and the web app kept their processes. The worker then saw the API at `http://127.0.0.1:20060`. A restart of one service applies its edits too, as the ready check above showed: the restart used the new path without a reload.

## On your own project

- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [Troubleshooting](../troubleshooting.md)
- [Config files and reload](../concepts/config-and-reload.md)
- [States and exit codes](../reference/states-and-exit-codes.md)
