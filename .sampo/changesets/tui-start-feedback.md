---
cargo/devlanes: patch
cargo/devlanes-tui: patch
---

Changed `s` in the TUI to mark the service queued at once, say what it waits for (`waiting for migrate-db`, `3rd in line`), ignore a second `s` on a queued service, and name start steps in the header as `containers` or the running task instead of raw phase names.
