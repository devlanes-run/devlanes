---
cargo/devlanes: patch
cargo/devlanes-runner-windows: patch
---

Fixed Windows units left by a previous daemon getting Ctrl-Break on `stop`, which skipped Node's `SIGINT` handlers.
