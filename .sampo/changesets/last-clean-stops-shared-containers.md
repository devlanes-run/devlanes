---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: patch
---

Changed `devlanes lane clean` of a project's last lane to stop its shared containers, keeping their data.

The next `devlanes start` in the project brings them up again.
