---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
---

Changed `devlanes lane new --start --output json` to print the new lane, then the start as `devlanes start --json` does.

Read the start's result from the last line instead of the `start` key. `--timeout` now applies to that start. A devlanes 0.12 client gets an error for `lane new --start` from this daemon; run `devlanes lane new`, then `devlanes start --lane <name>`.
