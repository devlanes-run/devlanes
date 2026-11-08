---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-mcp: patch
---

Fixed `devlanes lane new --start` and `daemon restart` through `devlanes mcp` starting lanes without the login shell's environment.
