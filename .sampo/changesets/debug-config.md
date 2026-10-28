---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
---

Added `devlanes debug config`, which prints a lane's project config as devlanes reads it, after `extends`, local overrides and templates, with the files it came from.
