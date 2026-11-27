---
cargo/devlanes: patch
cargo/devlanes-transport: patch
cargo/devlanes-daemon: patch
cargo/devlanes-tui: patch
---

Changed a box that stays unreachable to be retried every 2 minutes instead of every 30 seconds.

`devlanes remote connect BOX`, or Enter on the box in the TUI's Boxes view, tries again at once.
