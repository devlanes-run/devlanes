---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Added step-by-step progress to `devlanes daemon stop` and `daemon restart` on a terminal, and the time each shutdown step took to `daemon.log`, with any process that took more than 2 seconds to stop.
