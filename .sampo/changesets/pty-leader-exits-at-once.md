---
cargo/devlanes: patch
cargo/devlanes-runner-pty: patch
cargo/devlanes-sys: patch
---

Fixed `devlanes start` on macOS sometimes waiting until its timeout, and `devlanes daemon stop` stalling after it, when a service printed something and exited right after it started, instead of reporting it as failed with its exit code and output.
