---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed a `lane new` lane that waits for `lanes.max` disappearing after a daemon restart.

`devlanes lane clean <name>` removes it, and cleaning a project's other lanes keeps it.
