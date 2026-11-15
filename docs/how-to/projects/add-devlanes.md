---
title: Add devlanes to a project
description: Write a project file for a project that already runs some other way, and start it as a lane with an HTTPS hostname per service.
kind: how-to
---

You need devlanes on your machine, as [Install devlanes](../../install.md) sets it up, and the ideas from [A service, a port and a hostname](../../learn/first-service.md).

Use this guide when your project starts today with a dev script or a few terminals. At the end it runs as a lane, each service on a port of its own and behind an HTTPS hostname. A coding agent with the devlanes skill can do all of it for you; [Set up a coding agent](../agents/set-up.md) gives it the skill.

::example{slug="basics-hello-node"}

## Name the project

Create `devlanes.yaml` at the root of the repository and give the project a name. The name goes into the lane's domain and is shared by every worktree, so choose one you will keep: renaming it later makes a new project with new lanes, as [Lanes and projects](../../concepts/lanes-and-projects.md) explains.

```yaml devlanes
project: myapp
```

## List each long-running command as a service

Each command you keep running in a terminal becomes a service. When the command is a script in `package.json`, a Makefile or a similar file, point `run_script` at it, and devlanes runs it with the project's tool from that file's folder.

```yaml devlanes
services:
  api:
    run_script:
      file: api/package.json
      name: dev
```

[Run scripts your project already has](run-scripts.md) covers the other file kinds and plain commands. A command that ends by itself, such as an install or a migration, belongs in a task, as [Run installs, migrations and seeds](../services/tasks.md) shows. A database or another container goes under `infra`, as in [Share one database server between lanes](../infra/shared-database.md).

## Make each service listen on its port

devlanes picks a port for each service in each lane and passes it in the `PORT` variable, with `HOST` set to the loopback address. A server that reads both needs no other change, and the fallbacks keep it working when you start it without devlanes. [Run Node, Bun, Deno, Python and Rust services](runtimes.md) has the same lines for other runtimes.

```js
// api/server.mjs
const port = Number(process.env.PORT ?? 3000);
const host = process.env.HOST ?? "127.0.0.1";

server.listen(port, host);
```

A server that listens on a port of its own never passes its ready check, because devlanes checks and routes the port it handed out. The start then fails in the first lane already.

## Give a hostname to what a browser opens

Give each service a browser opens a hostname under the lane's domain. The expression `${{ lane.domain }}` is a different domain in each lane, so one line gives the service a hostname in every lane. A queue worker can do without one.

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
```

## Add a ready check

Without a ready check, a service with a hostname counts as ready once its port accepts a connection, which can happen before the app works. Use a route your app already answers with a 2xx status only when it works, such as a health route or `/`; any other status keeps the start waiting. [Wait until a service is ready](../services/ready-checks.md) shows the other kinds of check.

```yaml devlanes
services:
  api:
    ready:
      http:
        path: /healthz
```

## Check the file

Put together, the steps above give this file.

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

The check does not open the scripts the file names, so a misspelt script name passes it and shows up at the start. Once the file checks, commit it, so every worktree of the repository has it.

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
