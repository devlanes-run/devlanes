---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-proto: patch
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
---

Fixed `devlanes with` leaving units running when Ctrl-C interrupted its start.

`with` needs a daemon of this release: run `devlanes daemon restart` when it says the daemon is outdated.
