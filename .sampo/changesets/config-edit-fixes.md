---
cargo/devlanes: patch
cargo/devlanes-agent-hooks: patch
cargo/devlanes-daemon: patch
---

Fixed config edits on running lanes: an invalid or deleted project file no longer takes running services off the proxy, saved profiles and services the config dropped are left out of a plain `start`, `lane resume` and `daemon restart` with a warning instead of failing them, edits to the primary worktree's `devlanes.local.yaml` and to `.env` are picked up, edited `infra.setup` entries and `lane.create` tasks run again, and `lane gc` keeps port offsets another worktree's config still uses.
