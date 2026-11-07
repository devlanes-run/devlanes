---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Changed a plain `devlanes start` to name only its own project's lanes waiting for `lane resume`.

It counts the other projects' waiting lanes, which `lane resume` also starts.
