---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `ready.log` patterns that start with `^` never matching on Windows services with `pty: true`.
