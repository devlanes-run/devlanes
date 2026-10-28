---
cargo/devlanes: patch
cargo/devlanes-tui: patch
---

Changed the TUI monitor to sort units by name by default and switch the sort with the left and right arrows (`h` and `l` still fold lanes), and to draw each row's CPU history as dotted braille with two seconds per cell.

The kind, peak and uptime headers now show their sort arrow instead of `…`, and uptimes of 10 hours or more no longer push the rest of the row out of line.
