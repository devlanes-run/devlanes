---
cargo/devlanes: patch
cargo/devlanes-tui: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-cli: patch
---

Added a read-only view of connected boxes to the TUI: each box's projects follow the local ones (`acme @hetzner`) with their units' ports, mirror state and the URLs that open them here, and `B` shows every box's connection, errors and warnings.
