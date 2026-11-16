---
cargo/devlanes: patch
cargo/devlanes-config: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proxy: patch
cargo/devlanes-transport: patch
---

Added a `Server-Timing` header to proxied responses that splits each request's time between devlanes and the service, turned off with `proxy.server_timing: false`.
