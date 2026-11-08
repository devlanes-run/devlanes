---
cargo/devlanes: patch
cargo/devlanes-mcp: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes mcp` reporting another lane's start as the progress of a start call.

Calls of `with` and `lane new --start` now report the phases of the lane they start too.
