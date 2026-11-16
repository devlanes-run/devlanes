---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes stop --logs-dir` and `devlanes with --logs-dir` writing empty log files for units that never printed anything.
