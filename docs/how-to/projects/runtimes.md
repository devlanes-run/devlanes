---
title: Run Node, Bun, Deno, Python and Rust services
description: The lines a Node, Bun, Deno, Python or Rust service needs to listen where devlanes tells it, reload when you edit it and trust the devlanes CA.
kind: how-to
---

Use this guide when you put a Node, Bun, Deno, Python or Rust service on devlanes and want the lines its code and command need.

## Listen on the port and host

Each service gets its port in `PORT` and the loopback address in `HOST`. Read both where the server starts listening.

::::tabs{sync="runtime"}
:::tab[Node]
```js
const port = Number(process.env.PORT ?? 3000);
server.listen(port, process.env.HOST ?? "127.0.0.1");
```
:::
:::tab[Bun]
```ts
Bun.serve({
  port: Number(process.env.PORT ?? 3000),
  hostname: process.env.HOST ?? "127.0.0.1",
  fetch,
});
```
:::
:::tab[Deno]
```ts
Deno.serve({
  port: Number(Deno.env.get("PORT") ?? 8000),
  hostname: Deno.env.get("HOST") ?? "127.0.0.1",
}, handler);
```

The task has to allow reading the two variables:

```json title="deno.json"
{ "tasks": { "dev": "deno run --watch --allow-net --allow-env=PORT,HOST main.ts" } }
```
:::
:::tab[Python]
```yaml devlanes
services:
  api:
    run: uv run uvicorn acme.main:app --host 127.0.0.1 --port ${{ self.port }} --reload
```
:::
:::tab[Rust]
```rust
let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
```
:::
::::

## Reload on change

devlanes leaves watching files to the runtime, so use its watch mode in the command:

- Node: `node --watch`, tsx watch or nodemon.
- Bun: `bun --watch`.
- Deno: `deno run --watch`, as in the task above.
- Python: uvicorn's `--reload`.

A Rust service rebuilds when you restart it:

```console
$ devlanes restart api
```

## If devlanes picks the wrong runtime

devlanes tells the runtime from the script's file or the service's folder. A folder with a package.json counts as Node, so set the runtime on a Python or Rust service there:

```yaml devlanes
services:
  api:
    platform: python
```

## If a service calls another hostname over HTTPS

devlanes points these runtimes at its CA, so remove any setting that turns certificate checks off. [Trust the devlanes CA](../hostnames/trust-the-ca.md) covers other runtimes.

## Related

- [Run scripts your project already has](run-scripts.md)
- [Wait until a service is ready](../services/ready-checks.md)
- [`services.<name>.platform`](/docs/reference/config/project/services/#services-name-platform)
