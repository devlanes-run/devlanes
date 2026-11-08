---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes logs -f --grep` and `--level` showing lines that arrive while following without filtering them.

Lines that arrive while following also keep their colors, as the first page does.
