---
cargo/devlanes: patch
cargo/devlanes-config: patch
---

Changed the default `updates.mode` to `auto`, so devlanes installs patch releases in the background.

To only see new releases and install them yourself, set `updates.mode: notify` in the user config.
