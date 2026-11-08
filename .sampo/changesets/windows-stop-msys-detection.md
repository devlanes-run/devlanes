---
cargo/devlanes: patch
cargo/devlanes-runner-windows: patch
cargo/devlanes-sys-windows: patch
---

Fixed Windows stops of `pty: false` units running Git's `sh` or another MSYS program waiting for `stop.timeout`.
