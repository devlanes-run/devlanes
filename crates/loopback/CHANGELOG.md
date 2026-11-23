# devlanes-loopback

## 0.2.1 — 2026-11-23

### Patch changes

- [1ed9cccc](https://github.com/devlanes-run/devlanes/commit/1ed9cccc55b9dc00d11434307ec3074ba99da3cf) Fixed small writes through box ports and the proxy, such as WebSocket messages, arriving late.

## 0.2.0 — 2026-11-09

### Patch changes

- [a5309033](https://github.com/devlanes-run/devlanes/commit/a5309033e8d296987a2dd797dc81c0be182ae5b0) Fixed the proxy on Windows taking 2 seconds per connection to services that listen only on `::1`, such as Vite on `localhost`.
  
  Readiness checks of such services, port checks of infra containers that are not listening yet and connections a Windows box makes to its own services no longer wait either.

