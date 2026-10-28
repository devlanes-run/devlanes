---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-config: patch
---

Changed `daemon.exit_when_idle` so a daemon that only commands used, with no TUI ever open, also exits once nothing has run for about 5 minutes; before, it kept running until stopped.
