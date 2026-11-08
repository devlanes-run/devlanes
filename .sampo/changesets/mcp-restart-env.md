---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
cargo/devlanes-mcp: patch
cargo/devlanes-tui: patch
cargo/devlanes-proto: minor
---

Fixed `devlanes restart <unit>` through `devlanes mcp` keeping an `--env` value from an earlier command.
