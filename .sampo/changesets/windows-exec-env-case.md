---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `exec --env` on Windows not replacing a value that waits on a container when the name differs only in case.
