---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes lane clean` leaving shared containers running when the primary worktree has no project file.

`devlanes infra down` also works from a lane's worktree in that setup.
