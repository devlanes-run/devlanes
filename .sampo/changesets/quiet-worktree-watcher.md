---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-git: patch
---

Fixed the daemon re-reading the worktree list and lane layouts on every write to a repository's `.git` folder.
