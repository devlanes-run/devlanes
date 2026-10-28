---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-tui: patch
---

Added `devlanes reload`, which applies project file and `.env` edits to a running lane by stopping removed services, restarting changed ones and starting added ones, and shows per service in `status` and the TUI what an edit changed. `devlanes restart` without services now restarts the whole lane.
