---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed edits to the project file of a running lane: an invalid or deleted file keeps its services on the proxy, saved profiles and services the file dropped are skipped with a warning, edits to the primary worktree's `devlanes.local.yaml` and to `.env` are detected, edited `infra.setup` entries and `lane.create` tasks run again, and `lane gc` keeps port offsets other worktrees still use.
