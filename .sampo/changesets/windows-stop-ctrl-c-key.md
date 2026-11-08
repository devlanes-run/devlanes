---
cargo/devlanes: patch
cargo/devlanes-core: patch
cargo/devlanes-runner-windows: patch
---

Changed Windows stops to type Ctrl-C into `pty: true` units that run Git's `sh`, so their programs shut down cleanly.

A program that reads its terminal raw and ignores Ctrl-C now runs until `stop.timeout`. `run:` strings with `stop.signal: SIGINT` no longer get a launch note.
