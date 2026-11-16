---
title: Run a service in its runtime
description: The lines a Node, Bun, Deno, Python, Rust, Go or Java service needs to listen where devlanes tells it, reload when you edit it and trust the devlanes CA.
kind: how-to
---

Use this guide when you put a Node, Bun, Deno, Python, Rust, Go or Java service on devlanes and want the lines its code and command need.

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
:::tab[Go]
```go
host := cmp.Or(os.Getenv("HOST"), "127.0.0.1")
port := cmp.Or(os.Getenv("PORT"), "8080")
log.Fatal(http.ListenAndServe(net.JoinHostPort(host, port), mux))
```
:::
:::tab[Java]
```java
int port = Integer.parseInt(System.getenv().getOrDefault("PORT", "8080"));
String host = System.getenv().getOrDefault("HOST", "127.0.0.1");
```

Spring Boot and Quarkus take the port from a preset instead, as [Run Spring Boot and Quarkus](spring-boot-and-quarkus.md) shows.
:::
::::

## Reload on change

devlanes leaves watching files to the runtime, so use its watch mode in the command:

- Node: `node --watch`, tsx watch or nodemon.
- Bun: `bun --watch`.
- Deno: `deno run --watch`, as in the task above.
- Python: uvicorn's `--reload`.
- Java: Quarkus dev mode (`quarkus:dev`, `quarkusDev`), or Spring Boot DevTools, which restarts the app when your IDE or a `mvn compile` writes new classes.
- Go: [air](https://github.com/air-verse/air), building into the lane's data folder so two lanes never run one binary. On Windows, end both paths in `api.exe`.

```yaml devlanes
services:
  api:
    run:
      - air
      - --build.cmd
      - go build -o "${{ lane.data_path }}/api" ./cmd/api
      - --build.entrypoint
      - ${{ lane.data_path }}/api
```

A Rust service, a Go service run with `go run`, or a Java service without dev mode or DevTools rebuilds when you restart it:

```console
$ devlanes restart api
```

## If devlanes picks the wrong runtime

devlanes tells the runtime from the script's file or the service's folder. A folder with a package.json counts as Node, and one with a Cargo.toml as Rust, so set the runtime on another service there:

```yaml devlanes
services:
  api:
    platform: python
```

A service counts as Java when its script is in, or its folder holds, a `pom.xml` or Gradle file. One that runs `java -jar` from a folder without either needs `platform: java`.

## If a service calls another hostname over HTTPS

devlanes points these runtimes at its CA, so remove any setting that turns certificate checks off. [Trust the devlanes CA](../hostnames/trust-the-ca.md) covers other runtimes.

Go programs built without cgo look lane hostnames up in DNS; [Fix programs that cannot resolve *.localhost](../hostnames/resolve-localhost.md) makes them resolve.

## Related

- [Run scripts your project already has](run-scripts.md)
- [Run Spring Boot and Quarkus](spring-boot-and-quarkus.md)
- [Wait until a service is ready](../services/ready-checks.md)
- [`services.<name>.platform`](/docs/reference/config/project/services/#services-name-platform)
