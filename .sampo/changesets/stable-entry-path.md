---
cargo/devlanes: patch
cargo/devlanes-client: patch
cargo/devlanes-update: patch
---

Fixed `devlanes daemon install` and restarts from a TUI that outlived an update starting a removed binary (a Homebrew Cellar path, or a replaced file on Linux); they now use the install's stable path.
