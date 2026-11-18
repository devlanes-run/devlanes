---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed the daemon keeping several CPU cores busy in projects with many lanes, even with nothing running.
