---
cargo/devlanes: patch
cargo/devlanes-proxy: patch
---

Fixed the proxy moving to its fallback port (8443 or 8080) on macOS and Linux when the daemon restarted shortly after the proxy had closed connections, such as a browser's after `devlanes daemon restart`.
