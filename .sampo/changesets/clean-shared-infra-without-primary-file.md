---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes lane clean` leaving shared containers running when the primary worktree has no project file.

It no longer reports that the project file does not load, and `devlanes infra down` works from a lane's worktree in that setup too.
