---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-transport: patch
cargo/devlanes-tui: patch
---

Added `devlanes expose <port> [--host <name>]`, which makes a port of the lane that devlanes does not run a unit (`port-<port>`), routes the hostname to it and mirrors it to connected clients until `devlanes expose --remove <port>`, a stop of the whole lane or a daemon restart.
