---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-tui: patch
---

Fixed the monitor (`M`) erasing the graph history of units, lanes and containers once they stopped or were cleaned; they now stay in the table as stopped until their samples leave the graph, and a stopped lane no longer counts toward the current CPU and memory totals.
