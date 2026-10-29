---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
cargo/devlanes-proto: patch
cargo/devlanes-state: minor
cargo/devlanes-proxy: patch
---

Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane that several boxes run.

A switch moves the lane's hostnames and ports to the other box at once and closes open connections to the old box. The same lane on two boxes is now mirrored from one of them, even when their hostnames differ. To work with both at once, run the lane under another lane number or project on one box. In the TUI, box lanes now join the lanes row of the local project with the same name, and lanes this machine does not mirror are grey. See [several boxes with the same project](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/remote-multiple-boxes.md).
