---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
cargo/devlanes-tui: minor
cargo/devlanes-proto: minor
cargo/devlanes-state: minor
cargo/devlanes-proxy: minor
---

Added `devlanes remote use` and the TUI's `m` key to choose which box serves a lane that several boxes run.

A switch moves the lane's hostnames and ports to the other box at once and closes open connections to the old box. The same lane on two boxes is now mirrored from one of them, even when their hostnames differ. In the TUI, box lanes now join the lanes row of the local project with the same name, and lanes this machine does not mirror are grey. See [several boxes with the same project](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/remote-multiple-boxes.md).
