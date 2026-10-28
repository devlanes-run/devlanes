# devlanes-git

## 0.7.0 — 2026-10-28

### Minor changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key; devlanes no longer creates worktrees.
  
  Create one with `git worktree add <path> <branch>` and run `devlanes start` in it. Configs that still set `lanes.worktree_dir` fail to load until it is removed. `devlanes lane new --name` still adds another lane for the current folder.

## 0.6.0 — 2026-10-28

### Patch changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed the daemon on Linux listing each project's Git worktrees several times a second while nothing changed.

## 0.5.0 — 2026-10-26

Version bump only, to follow other packages.

## 0.4.1 — 2026-10-26

### Patch changes

- [6545d4b](https://github.com/devlanes-run/devlanes/commit/6545d4bc38c1ea672a383bf1eb0c5e0077bf8adb) Fixed `git worktree move` failing with "Permission denied" on Windows while the devlanes daemon watched the repository.

## 0.4.0 — 2026-10-25

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Fixed a worktree whose `.git` entry went missing being registered as the main checkout.

