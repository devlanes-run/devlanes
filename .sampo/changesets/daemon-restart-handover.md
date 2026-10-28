---
cargo/devlanes: minor
cargo/devlanes-client: minor
cargo/devlanes-daemon: minor
cargo/devlanes-proto: minor
---

Changed `devlanes daemon restart` to finish as soon as the new daemon answers, with the services that ran coming back as ordinary starts; once the old daemon was asked to stop, the restart completes even if the terminal or TUI that asked goes away.

`daemon restart --json` now lists the lanes it starts again in `resuming` instead of their start results in `resumed`; use `devlanes status` or `devlanes wait` to follow them.
