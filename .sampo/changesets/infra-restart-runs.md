---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes status` keeping an infra container's old ports after it crashed twice in a row or an event was lost.
