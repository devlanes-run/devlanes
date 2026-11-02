---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-tui: patch
cargo/devlanes-config: patch
---

Changed `daemon.exit_when_idle` to stop the daemon about 15 seconds after the last TUI closes instead of 30.

The summary printed when the TUI closes now says `stopping in 15s` instead of `running` when nothing keeps the daemon up.
