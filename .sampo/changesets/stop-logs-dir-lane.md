---
cargo/devlanes: patch
---

Fixed `devlanes stop --logs-dir` without `--lane` failing with `invalid reference`; it now stops the current worktree's lane, as plain `devlanes stop` does.
