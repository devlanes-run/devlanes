---
cargo/devlanes: minor
cargo/devlanes-state: minor
cargo/devlanes-daemon: minor
---

Changed `devlanes debug errors` to never count a requested stop as a crash, and to count deaths by signal and task timeouts.

The state database gains a column, so an older devlanes cannot open it. Runs recorded before the upgrade are not counted.
