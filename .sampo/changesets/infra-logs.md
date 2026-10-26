---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-infra-docker: patch
cargo/devlanes-proto: patch
cargo/devlanes-tui: patch
---

Added `devlanes infra logs <container> [-f]` and Docker logs in the TUI pane when an infra container is selected, streamed from Docker and kept across container restarts.
