---
title: A service, a port and a hostname
description: The API of myapp starts under devlanes and answers on an HTTPS hostname.
kind: learn
---

myapp starts as one API with a dev script. Under devlanes that script becomes a service with a port of its own and an HTTPS hostname, and it stays up when the terminal that started it goes away.

## One file describes the project

The file is `devlanes.yaml`. It sits at the root of the repository and is committed like the rest of the code. It names the project and lists its services, the long-running processes the project is made of.

```yaml devlanes
# devlanes.yaml
project: myapp

services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
```

The file says which script starts the API, which hostname it wants and how to tell that it is ready. That last part is a ready check: a request devlanes repeats until the service answers. In the hostname, `${{ lane.domain }}` is an expression that devlanes replaces with the lane's domain, `myapp.localhost` here. No port appears anywhere.

## The service listens where it is told

devlanes picks a port for each service and passes it in the `PORT` variable. Reading it is the one thing the project has to do.

```js
// api/server.mjs
const port = Number(process.env.PORT ?? 3000);
const host = process.env.HOST ?? "127.0.0.1";

server.listen(port, host, () => {
  console.log(`api listening on http://${host}:${port}`);
});
```

A dev server that takes its port as a flag gets it another way, which [Services that find each other](connecting-services.md) shows.

## A start runs the script and waits for it

A start runs the dev script with that port, repeats the ready check until the API answers, then prints where to find it.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
```

The second line reads as lane 0 of myapp, on the main branch. A lane is one worktree's place to run the project, with a number, ports and hostnames of its own; [Every worktree gets a lane](worktrees.md) adds another.

On an interactive terminal the start runs inside the TUI, a full-screen view of each lane and its output. The `--no-ui` flag prints the same start as text, which is the form these pages show. Starting again is safe: a second start finds the API running and starts nothing.

## The daemon owns the process, not the terminal

The start handed the API to a background daemon and returned. The terminal it was typed in has no part in keeping the API up.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  1/1       0.0%  38.6 MiB  https://myapp.localhost

  UNIT  STATE  PORT   CPU   MEM       URL
  api   ready  20060  0.0%  38.6 MiB  https://api.myapp.localhost
```

The first table is the lane. Its URL is the lane's domain, where nothing answers yet: the API asked for a hostname under it, not for the domain itself. The second table lists the lane's units, the things devlanes runs in a lane, of which a service is one kind: here only the API, ready on port 20060 behind the hostname the project file asked for. A stop ends the process and keeps both the port and the hostname for the next start.

## The output is kept

What a service prints goes to the daemon, which keeps it. A terminal opened later can read it back.

```console title="~/dev/myapp"
$ devlanes logs api --tail 10
api#1 api listening on http://127.0.0.1:20060
```

The prefix `api#1` is the unit's name and the number of the line in its log, here the first, and a later read can pick up after that line with `--cursor api#1`.

When the checkout branch gets a worktree of its own, the same project file starts the API a second time, on other ports and another hostname.

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Run scripts your project already has](../how-to/projects/run-scripts.md)
- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [The daemon](../concepts/daemon.md)
- [Hostnames and HTTPS](../concepts/hostnames-and-https.md)
