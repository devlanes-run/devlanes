---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-config: minor
cargo/devlanes-daemon: minor
cargo/devlanes-git: minor
---

Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key; devlanes no longer creates worktrees.

Create one with `git worktree add <path> <branch>` and run `devlanes start` in it. Configs that still set `lanes.worktree_dir` fail to load until it is removed. `devlanes lane new --name` still adds another lane for the current folder.
