---
title: Add devlanes to a project
description: Write a project file for a project that already runs some other way, and start it as a lane with an HTTPS hostname per service.
kind: how-to
---

You need devlanes on your machine, as [Install devlanes](../../install.md) sets it up, and the ideas from [A service, a port and a hostname](../../learn/first-service.md).

Use this guide when your project starts today with a dev script or a few terminals. At the end it runs as a lane, each service on a port of its own and behind an HTTPS hostname. A coding agent with the devlanes skill can do all of it for you; [Set up a coding agent](../agents/set-up.md) gives it the skill.

::example{slug="basics-hello-node"}

## Name the project

Create `devlanes.yaml` at the root of the repository and give the project a name. The name goes into every hostname and is shared by every worktree, so choose one you will keep: renaming it later makes a new project with new lanes, as [Lanes and projects](../../concepts/lanes-and-projects.md) explains.

```yaml devlanes
project: myapp
```

## List each long-running command as a service

Each command you keep running in a terminal becomes a service. When the command is a script in `package.json` or another project file, point `run_script` at it, and devlanes runs it with the project's tool from that file's folder.

```yaml devlanes
services:
  api:
    run_script:
      file: api/package.json
      name: dev
```

[Run scripts your project already has](run-scripts.md) covers the other file kinds and plain commands. A command that ends by itself, such as an install or a migration, belongs in a task, as [Run installs, migrations and seeds](../services/tasks.md) shows. A database or another container goes under `infra`, as in [Share one database server between lanes](../infra/shared-database.md).

## Make each service listen on its port

devlanes picks a port for each service in each lane and passes it in the `PORT` variable, with `HOST` set to the loopback address. A server that reads both needs no other change, and the fallbacks keep it working when you start it without devlanes.

```js
// api/server.mjs
const port = Number(process.env.PORT ?? 3000);
const host = process.env.HOST ?? "127.0.0.1";

server.listen(port, host);
```

A port written into the code or the script makes the second lane fail, because both lanes try to take it.

## Give a hostname to what a browser opens

The lane's domain is `myapp.localhost` in lane 0 and `myapp1.localhost` in lane 1, so one line gives the service a hostname in every lane. A service nobody opens in a browser, such as a queue worker, can do without one.

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
```

## Add a ready check

Without a ready check, a service with a hostname counts as ready once its port accepts a connection, which can happen before the app works. A route that answers only when the app is up makes the start wait for the real thing. [Wait until a service is ready](../services/ready-checks.md) shows the other kinds of check.

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
```

## Check the file

Put together, the steps above give this file. Commit it, so every worktree of the repository has it.

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

The check reads the file without starting anything. A mistake comes back with its line number and, for a misspelt name, the name it should be.

```console title="on Windows 11"
$ devlanes config check
ok myapp (C:\Users\you\dev\myapp\devlanes.yaml)
```

## Start the lane

The start runs the script, waits for the ready check to pass and prints the hostname to open.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
```

A script name the file does not have fails the start, and the error lists the scripts the file does have.

## If the command takes its port as a flag

Pass the port in the command with an expression. For Vite and Next.js a preset adds the flags, as [Run Vite and Next.js](vite-and-next.md) shows.

```yaml devlanes
services:
  api:
    run: node server.mjs --port ${{ self.port }}
```

## If one service calls another on a fixed port

Replace the fixed address with the other service's URL in the same lane, as [Services that find each other](../../learn/connecting-services.md) does for a web app and its API.

## If another tool runs the project today

[Migrate from another process manager](migrate.md) maps mprocs, a Procfile, Compose and similar files to a project file.

## Related

- [Run Node, Bun, Deno, Python and Rust services](runtimes.md)
- [Read .env values and pin tool versions](env-files-and-tools.md)
- [Keep personal changes out of Git](local-overrides.md)
- [`services.<name>.run_script`](/docs/reference/config/project/services/)
