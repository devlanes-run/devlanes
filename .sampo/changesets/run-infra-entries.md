---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-tui: patch
---

Added `devlanes run infra.setup.<name>` and `devlanes run infra.teardown.<name>` to run one setup or teardown entry, and fixed `s` and `r` on a setup step in the TUI, which failed with "no lane or unit named setup.<name>" and now run that entry.
