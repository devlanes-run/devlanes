---
cargo/devlanes: minor
cargo/devlanes-state: minor
cargo/devlanes-daemon: minor
---

Changed `devlanes debug errors` to count a crash only when a unit ended without a stop asking for it, including deaths by signal.

The state database gains a column, so an older devlanes cannot open it. Runs recorded before the upgrade are not counted.
