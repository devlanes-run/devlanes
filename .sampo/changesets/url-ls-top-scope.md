---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Changed `devlanes url ls`, `debug top` and `status --watch` to cover only the current project, or the one `-p` names.

Outside every project they cover all of them. `status --watch` lines count each lane's units by state, and agent output prints them.
