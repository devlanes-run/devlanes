---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-resolve: patch
---

Fixed a service put back in the project file not showing as added when it had exited on its own.

`devlanes wait` and `devlanes reload` answer not found for a service the project file no longer has once it stopped.
