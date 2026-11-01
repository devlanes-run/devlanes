---
cargo/devlanes: patch
cargo/devlanes-runner-windows: patch
---

Fixed `devlanes attach` and the TUI's unit pane on Windows showing a `pty: true` unit's lines with gaps, and missing later resizes, after a resize that came while the unit was printing.
