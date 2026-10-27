---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-transport: patch
cargo/devlanes-tunnel: patch
---

Fixed `devlanes remote connect` mirroring more than it should: boxes no longer send the names of `hidden` units or sync a hidden service's `sync` folders, a port a local program holds on one loopback address (on Windows, also the wildcard address) is no longer mirrored, and a box can no longer claim `localhost`, `*.localhost` or names under a local wildcard.
