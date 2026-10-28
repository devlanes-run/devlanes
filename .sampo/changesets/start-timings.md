---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-resolve: patch
---

Added timings for every start step to `-v` output, with repeated steps added up into one row, and a `start finished` line per start in the daemon log that names its slowest phases and start slot waits.
