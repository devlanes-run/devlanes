---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-config: patch
---

Changed the lane of a deleted worktree to stay, with its data, containers and ports, until `devlanes lane gc --yes` cleans it; set `lanes.removed_worktrees: clean` to have the daemon clean it as before.
