---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes lane clean` saying the project file does not load when the primary worktree has none.

It now stops the shared containers the lane's own file declares, and `devlanes infra down` works from a lane's worktree in that setup.
