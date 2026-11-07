---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes lane new --name` refusing a cleaned lane's name.

The name now gives a new lane with empty data. A taken name gets its own error, `main` and `lane<N>` are refused,
and `lane new` prints the project folder instead of the repository root.
