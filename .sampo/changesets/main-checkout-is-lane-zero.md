---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-config: minor
---

Changed lane 0 to be the main checkout on any branch, instead of the worktree on the primary branch.

A linked worktree that held lane 0 moves to the lowest free number at its next start while nothing runs in it, so its hostnames, `lane.name` and values built from them change. Only a repository without a main checkout, such as a bare clone, still gives lane 0 to the worktree on `lanes.primary`.
