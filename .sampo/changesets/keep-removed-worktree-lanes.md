---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-config: patch
---

Changed a deleted worktree's lane to keep its data and ports until `devlanes lane gc --yes` cleans it.

Set `lanes.removed_worktrees: clean` to have the daemon clean these lanes by itself, as before.
