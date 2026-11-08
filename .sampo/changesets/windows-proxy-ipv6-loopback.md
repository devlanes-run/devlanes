---
cargo/devlanes: patch
cargo/devlanes-proxy: patch
cargo/devlanes-loopback: patch
cargo/devlanes-sys-windows: patch
cargo/devlanes-daemon: patch
cargo/devlanes-transport: patch
---

Fixed the proxy on Windows taking 2 seconds per connection to services that listen only on `::1`, such as Vite on `localhost`.

Readiness checks of such services and connections a Windows box makes to its own services no longer wait either.
