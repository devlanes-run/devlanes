---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes status` and `start` right after an edit to `devlanes.yaml` sometimes reading the new file before its ports and drift were ready.
