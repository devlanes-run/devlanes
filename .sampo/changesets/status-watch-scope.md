---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes status --watch` printing other projects' lanes and never showing queued units.

Each line now counts the lane's units by state, and agent output prints the lines instead of nothing.
