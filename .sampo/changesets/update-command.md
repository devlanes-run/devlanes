---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
cargo/devlanes-update: patch
---

Added `devlanes update` and the TUI's `U` key, which install the latest release the way devlanes was installed, restart the daemon and bring its lanes back.

`--next` and `--stable` switch between the latest build of main and stable releases. `u` no longer hides the release notice.
