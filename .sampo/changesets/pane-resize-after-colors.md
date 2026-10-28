---
cargo/devlanes: patch
cargo/devlanes-runner-windows: patch
cargo/devlanes-tui: patch
---

Fixed `pty: true` units on Windows whose output leaves a color active stopping to follow the TUI pane's size, which left a gap of empty rows above the cursor and blank lines in the log, and a scrolled-back pane jumping up when the unit's screen got shorter.
