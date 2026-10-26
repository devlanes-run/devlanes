---
cargo/devlanes: patch
cargo/devlanes-tui: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-cli: patch
---

Added connected boxes to the TUI, read-only: their projects get tabs after the local ones (`acme @hetzner`), a box unit's pane shows its ports, mirror state and the URLs that open it here, and `B` lists every box with its connection, errors and warnings.

`devlanes remote ls --json` entries also carry the box's `registry`, each port's `state` and each route's `url`.
