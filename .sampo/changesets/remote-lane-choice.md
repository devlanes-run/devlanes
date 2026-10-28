---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
cargo/devlanes-proto: patch
cargo/devlanes-state: patch
---

Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane that several boxes run.

In the TUI, box lanes now join the lanes row of the local project with the same name, and lanes this machine does not mirror are grey. See [several boxes with the same project](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/remote-multiple-boxes.md).
