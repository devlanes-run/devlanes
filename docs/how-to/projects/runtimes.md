---
title: Run Node, Bun, Deno, Python and Rust services
description: The lines a Node, Bun, Deno, Python or Rust service needs to listen where devlanes tells it, trust the devlanes CA and reload when you edit it.
kind: how-to
---

Use this guide when you put a Node, Bun, Deno, Python or Rust service on devlanes and want to know what its code and command need. devlanes tells which runtime a service uses from its script's file or its folder, and sets the variables that runtime reads.

## Listen on the port and host

Each service gets its port in `PORT` and the loopback address in `HOST`. Read both where the server starts listening, with fallbacks for when you run it without devlanes.

::::tabs
:::tab[Node]
```js
const port = Number(process.env.PORT ?? 3000);
server.listen(port, process.env.HOST ?? "127.0.0.1");
```

Express, Fastify, Hono and Koa take the same two values in their listen call.
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

Deno asks for permission to read the two variables, so the task grants it:

```json
{ "tasks": { "dev": "deno run --watch --allow-net --allow-env=PORT,HOST main.ts" } }
```
:::
:::tab[Python]
Python dev servers take the port and host as flags, so the service is a plain command with an expression for the port.

```yaml devlanes
services:
  api:
    run: uv run uvicorn acme.main:app --host 127.0.0.1 --port ${{ self.port }} --proxy-headers
```

The last flag makes request URLs read as HTTPS behind the proxy. Django and Flask need settings of their own for that, which [Set CORS and cookies for each lane](../hostnames/cors-and-cookies.md) shows.
:::
:::tab[Rust]
```rust
let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
let listener = tokio::net::TcpListener::bind(format!("{host}:{port}")).await?;
```
:::
::::

## Trust the devlanes CA

A service that calls another lane hostname over HTTPS needs to trust the devlanes CA. devlanes points each runtime at it through the variable that runtime reads, so you can delete any setting that turns certificate checks off.

::::tabs
:::tab[Node]
Node reads `NODE_EXTRA_CA_CERTS` once at startup. A script that sets this variable itself replaces the devlanes CA, so remove that line.
:::
:::tab[Bun]
Bun reads `NODE_EXTRA_CA_CERTS`, as Node does.
:::
:::tab[Deno]
Deno adds the file in `DENO_CERT` to its own roots.
:::
:::tab[Python]
`SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` name a bundle of the system roots and the devlanes CA. The standard library, requests, httpx and aiohttp each read one of them.
:::
:::tab[Rust]
`SSL_CERT_FILE` names a bundle of the system roots and the devlanes CA. OpenSSL and rustls-native-certs read it, while a client built only on webpki-roots cannot trust the CA at all.
:::
::::

[Trust the devlanes CA](../hostnames/trust-the-ca.md) covers the other TLS stacks and the tools outside a lane.

## Reload on change

devlanes runs the command you give it and leaves watching files to the runtime, so put the runtime's watch flag in the script.

::::tabs
:::tab[Node]
Use `node --watch`, tsx watch or nodemon in the dev script.
:::
:::tab[Bun]
Use `bun --watch` in the dev script.
:::
:::tab[Deno]
The `--watch` flag in the task above restarts the server on each edit.
:::
:::tab[Python]
Add `--reload` to uvicorn, with `--reload-dir src` so the watcher skips the virtual environment. Django's runserver reloads by itself.
:::
:::tab[Rust]
Nothing rebuilds the binary by itself. After an edit, `devlanes restart api` runs cargo again and starts the new build.

To rebuild on every save, run cargo-watch or bacon as the service's command. The first start compiles every dependency, so give the ready check a long timeout.
:::
::::

A watcher keeps running after the program under it crashes. Give the service a ready check that reaches the app, as [Wait until a service is ready](../services/ready-checks.md) shows, so a crash on start does not pass as ready.

## If devlanes picks the wrong runtime

A folder with a `package.json` counts as Node even when it also holds a Python or Rust project file. Set the runtime on the service:

```yaml devlanes
services:
  api:
    platform: python
    cwd: services/api
    run: uv run uvicorn acme.main:app --host 127.0.0.1 --port ${{ self.port }}
```

A Bun project that has a `bunfig.toml` but no Bun lockfile runs as Bun, while its scripts go to npm. Name Bun in the `packageManager` field of its package.json, which settles both.

## If your runtime is not one of these

Units of any runtime get the CA in `DEVLANES_CA_CERT` and the bundle of system roots and CA in `DEVLANES_CA_BUNDLE`. Point the runtime's own setting at one of them. [Environment variables](../../reference/environment-variables.md) lists both.

## Related

- [Run scripts your project already has](run-scripts.md)
- [Read .env values and pin tool versions](env-files-and-tools.md)
- [`services.<name>.platform`](/docs/reference/config/project/services/)
