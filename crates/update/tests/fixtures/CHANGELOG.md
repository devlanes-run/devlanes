# devlanes

## 0.9.0 — 2026-10-29

### Minor changes

- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane.
  
  A switch moves the lane's hostnames and ports to the other box at once.
  
  See the guide.
- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Changed default lane domains to `<project><N>.localhost`, such as `acme2.localhost`.
  
  Lane 0 keeps `acme.localhost`.

### Patch changes

- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Added the `domain(name)` expression function.

## 0.8.1 — 2026-10-28

### Patch changes

- [effb343](https://github.com/devlanes-run/devlanes/commit/effb343649781d8c516285391605f27d192c66c0) Fixed `pty: true` units on Windows losing the pane's size.

## 0.8.0 — 2026-10-28

### Minor changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key.
  
  Create one with `git worktree add <path> <branch>` and run `devlanes start` in it.

### Patch changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added `devlanes reload`, which applies project file edits to a running lane.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added a top-level `extends` key that builds a project file on others.
  
  ```yaml
  # devlanes.prod.yaml
  extends: devlanes.yaml
  profiles:
    default: { include: ["*"] }
  ```

## 0.7.1 - 2026-10-28

### Patch changes

- Fixed CI checkouts getting lane 1.
