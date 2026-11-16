---
title: Add devlanes to a project
description: Write a project file for a project that already runs some other way, and start it as a lane with an HTTPS hostname per service.
kind: how-to
---

You need devlanes on your machine, as [Install devlanes](../../install.md) sets it up, and the ideas from [A service, a port and a hostname](../../learn/first-service.md).

Use this guide when your project starts today with a dev script or a few terminals. A coding agent with the devlanes skill can do all of it for you; [Set up a coding agent](../agents/set-up.md) gives it the skill.

::example{slug="basics-hello-node"}

## Write the project file

Create `devlanes.yaml` at the root of the repository. Each long-running command becomes a service that points at its script, a service you open in a browser gets a hostname, and the ready check names a route that answers once the app works.

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

The project name goes into the lane's domain and is shared by every worktree, so choose one you will keep. [Run scripts your project already has](run-scripts.md) covers other script files and plain commands.

## Listen on the port devlanes gives

devlanes passes each service its port in `PORT` and the loopback address in `HOST`. Read both where the server starts listening. A server that keeps a fixed port of its own never passes its ready check.

```js title="api/server.mjs"
const port = Number(process.env.PORT ?? 3000);
const host = process.env.HOST ?? "127.0.0.1";

server.listen(port, host);
```

[Run a service in its runtime](runtimes.md) has the same lines for other runtimes.

## Check and start

```console
$ devlanes config check
$ devlanes start --no-ui
```

The check points at the line of any mistake. The start runs the script, waits for the ready check and prints the hostname to open. Commit the file, so every worktree of the repository has it.

## If the command takes its port as a flag

Pass the port with an expression. For Vite and Next.js a preset adds the flags, as [Run Vite and Next.js](vite-and-next.md) shows, and for Spring Boot and Quarkus a preset sets the variables, as [Run Spring Boot and Quarkus](spring-boot-and-quarkus.md) shows.

```yaml devlanes
services:
  api:
    run: node server.mjs --port ${{ self.port }}
```

## If another tool runs the project today

[Migrate from another process manager](migrate.md) turns an mprocs, Procfile or Compose setup into a project file.

## Related

- [Run installs, migrations and seeds](../services/tasks.md)
- [Wait until a service is ready](../services/ready-checks.md)
- [Keep personal changes out of Git](local-overrides.md)
- [`services.<name>.run_script`](/docs/reference/config/project/services/#services-name-run_script)
