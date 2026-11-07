---
cargo/devlanes: minor
cargo/devlanes-cli: patch
cargo/devlanes-daemon: minor
---

Changed `devlanes wait` on an exec unit to wait until its command exits.

A server started with `devlanes exec` never exits, so `wait` on it now times out with exit 124; read `devlanes logs NAME` or open its URL instead.
`wait` on a task or exec unit that exited 0 now exits 0 instead of 1, and a timeout while it still runs says so.
