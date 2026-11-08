---
cargo/devlanes: patch
cargo/devlanes-proxy: patch
---

Fixed the proxy on Windows taking 2 seconds per connection to services that listen only on `::1`, such as Vite on `localhost`.
