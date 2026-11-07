---
cargo/devlanes: patch
cargo/devlanes-config: patch
cargo/devlanes-resolve: patch
cargo/devlanes-daemon: patch
---

Fixed `run` lists failing to start when an item is one `${{ }}` expression that gives a number, such as `self.port`.
