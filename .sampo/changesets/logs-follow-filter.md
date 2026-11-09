---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-logs: patch
---

Fixed `devlanes logs -f --grep` and `--level` showing lines that arrive while following without filtering them.

Lines that arrive while following also keep their colors and get the `--context` lines around matches, as the first page does.
