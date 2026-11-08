---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-state: minor
cargo/devlanes-cli: patch
cargo/devlanes-mcp-ui: patch
---

Changed `devlanes lane clean` so a lane that comes back starts empty, without the old `--env` or history.

The primary and worktree lanes get a new data folder path, and `lane clean --dry-run` lists the run history. The daemon migrates its database on the next start and deletes the history of lanes cleaned before. `devlanes lane gc` deletes lane data folders a clean could not delete.
