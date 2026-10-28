---
cargo/devlanes: patch
cargo/devlanes-tui: patch
---

Changed the TUI while the daemon restarts, stops or does not answer: every view shows what is happening above the live daemon log, the `devlanes` logo turns grey or red, and the view comes back once the daemon answers. Confirm dialogs no longer repeat their question as a title.
